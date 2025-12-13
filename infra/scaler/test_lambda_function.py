import pytest
import json
from unittest.mock import Mock, MagicMock, patch
from datetime import datetime, timezone, timedelta

from lambda_function import (
    get_queue_depth,
    required_workers,
    get_current_workers,
    get_last_scaling_time,
    is_in_cooldown,
    scale,
    lambda_handler,
    COOLDOWN_MINUTES
)


class TestGetQueueDepth:
    def test_get_queue_depth(self):
        """Test getting queue depth from database."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        mock_conn.cursor.return_value.__enter__.return_value = mock_cursor
        mock_cursor.fetchone.return_value = {'count': 42}

        result = get_queue_depth(mock_conn)

        assert result == 42
        mock_cursor.execute.assert_called_once()

    def test_get_queue_depth_no_results(self):
        """Test getting queue depth when no results."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        mock_conn.cursor.return_value.__enter__.return_value = mock_cursor
        mock_cursor.fetchone.return_value = None

        result = get_queue_depth(mock_conn)

        assert result == 0


class TestRequiredWorkers:
    def test_required_workers_small_queue(self):
        """Test required workers for small queue."""
        assert required_workers(0) == 1
        assert required_workers(5) == 1
        assert required_workers(3) == 1

    def test_required_workers_large_queue(self):
        """Test required workers for larger queues."""
        # log2(10) ≈ 3.32, ceil = 4, * 2 = 8
        assert required_workers(10) == 8
        # log2(20) ≈ 4.32, ceil = 5, * 2 = 10
        assert required_workers(20) == 10
        # log2(100) ≈ 6.64, ceil = 7, * 2 = 14
        assert required_workers(100) == 14


class TestGetCurrentWorkers:
    def test_get_current_workers(self):
        """Test getting current workers from ASG."""
        mock_client = MagicMock()
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': [{
                'DesiredCapacity': 5
            }]
        }

        result = get_current_workers(mock_client, 'test-asg')

        assert result == 5
        mock_client.describe_auto_scaling_groups.assert_called_once_with(
            AutoScalingGroupNames=['test-asg']
        )

    def test_get_current_workers_not_found(self):
        """Test error when ASG not found."""
        mock_client = MagicMock()
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': []
        }

        with pytest.raises(ValueError, match="not found"):
            get_current_workers(mock_client, 'test-asg')

    def test_get_current_workers_no_capacity(self):
        """Test error when desired capacity not set."""
        mock_client = MagicMock()
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': [{}]
        }

        with pytest.raises(ValueError, match="Desired capacity not set"):
            get_current_workers(mock_client, 'test-asg')


class TestGetLastScalingTime:
    def test_get_last_scaling_time_with_datetime(self):
        """Test getting last scaling time with datetime object."""
        mock_client = MagicMock()
        test_time = datetime(2024, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': test_time
            }]
        }

        result = get_last_scaling_time(mock_client, 'test-asg')

        assert result == test_time
        mock_client.describe_scaling_activities.assert_called_once_with(
            AutoScalingGroupName='test-asg',
            MaxRecords=1
        )

    def test_get_last_scaling_time_naive_datetime(self):
        """Test getting last scaling time with naive datetime."""
        mock_client = MagicMock()
        naive_time = datetime(2024, 1, 1, 12, 0, 0)
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': naive_time
            }]
        }

        result = get_last_scaling_time(mock_client, 'test-asg')

        assert result.tzinfo == timezone.utc
        assert result.replace(tzinfo=None) == naive_time

    def test_get_last_scaling_time_with_string(self):
        """Test getting last scaling time with ISO string."""
        mock_client = MagicMock()
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': '2024-01-01T12:00:00Z'
            }]
        }

        result = get_last_scaling_time(mock_client, 'test-asg')

        assert isinstance(result, datetime)
        assert result.tzinfo == timezone.utc

    def test_get_last_scaling_time_no_activities(self):
        """Test when no scaling activities exist."""
        mock_client = MagicMock()
        mock_client.describe_scaling_activities.return_value = {
            'Activities': []
        }

        result = get_last_scaling_time(mock_client, 'test-asg')

        assert result is None

    def test_get_last_scaling_time_exception(self):
        """Test handling exceptions when getting scaling activities."""
        mock_client = MagicMock()
        mock_client.describe_scaling_activities.side_effect = Exception("API error")

        result = get_last_scaling_time(mock_client, 'test-asg')

        assert result is None


class TestIsInCooldown:
    def test_is_in_cooldown_no_previous_scaling(self):
        """Test cooldown check when no previous scaling exists."""
        mock_client = MagicMock()
        mock_client.describe_scaling_activities.return_value = {
            'Activities': []
        }

        result = is_in_cooldown(mock_client, 'test-asg')

        assert result is False

    def test_is_in_cooldown_recent_scaling(self):
        """Test cooldown check when scaling happened recently."""
        mock_client = MagicMock()
        # Scaling happened 5 minutes ago (within 10 minute cooldown)
        recent_time = datetime.now(timezone.utc) - timedelta(minutes=5)
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': recent_time
            }]
        }

        result = is_in_cooldown(mock_client, 'test-asg')

        assert result is True

    def test_is_in_cooldown_old_scaling(self):
        """Test cooldown check when scaling happened long ago."""
        mock_client = MagicMock()
        # Scaling happened 45 minutes ago (outside 10 minute cooldown)
        old_time = datetime.now(timezone.utc) - timedelta(minutes=45)
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': old_time
            }]
        }

        result = is_in_cooldown(mock_client, 'test-asg')

        assert result is False

    def test_is_in_cooldown_exactly_10_minutes(self):
        """Test cooldown check at exactly 10 minutes boundary."""
        mock_client = MagicMock()
        # Scaling happened exactly 10 minutes ago
        exact_time = datetime.now(timezone.utc) - timedelta(minutes=COOLDOWN_MINUTES)
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': exact_time
            }]
        }

        result = is_in_cooldown(mock_client, 'test-asg')

        # Should be False since it's exactly at the boundary (not less than)
        assert result is False


class TestScale:
    @patch('lambda_function.get_queue_depth')
    @patch('lambda_function.get_current_workers')
    @patch('lambda_function.is_in_cooldown')
    def test_scale_in_cooldown_scale_down(self, mock_is_cooldown, mock_get_workers, mock_get_queue):
        """Test scaling down when in cooldown period (should be blocked)."""
        mock_conn = MagicMock()
        mock_client = MagicMock()

        mock_is_cooldown.return_value = True
        mock_get_workers.return_value = 5  # Current workers
        mock_get_queue.return_value = 5  # Requires 1 worker (queue <= 5), so target is 1
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': [{
                'MinSize': 1,
                'MaxSize': 10
            }]
        }
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': datetime.now(timezone.utc) - timedelta(minutes=5)
            }]
        }

        result = scale(mock_conn, mock_client, 'test-asg')

        assert result['scaled'] is False
        assert result['in_cooldown'] is True
        assert result['current_workers'] == 5
        assert result['target_capacity'] == 5  # Kept at current (cooldown blocked scale down)
        assert 'last_scaling_time' in result
        assert 'remaining_cooldown_seconds' in result
        # Should not call update_auto_scaling_group
        mock_client.update_auto_scaling_group.assert_not_called()

    @patch('lambda_function.get_queue_depth')
    @patch('lambda_function.get_current_workers')
    @patch('lambda_function.is_in_cooldown')
    def test_scale_in_cooldown_scale_up(self, mock_is_cooldown, mock_get_workers, mock_get_queue):
        """Test scaling up when in cooldown period (should be allowed)."""
        mock_conn = MagicMock()
        mock_client = MagicMock()

        mock_is_cooldown.return_value = True
        mock_get_workers.return_value = 2  # Current workers
        mock_get_queue.return_value = 20  # Requires more workers, so target > 2
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': [{
                'MinSize': 1,
                'MaxSize': 10
            }]
        }
        mock_client.describe_scaling_activities.return_value = {
            'Activities': [{
                'StartTime': datetime.now(timezone.utc) - timedelta(minutes=5)
            }]
        }

        result = scale(mock_conn, mock_client, 'test-asg')

        # Should scale up despite cooldown
        assert result['scaled'] is True
        assert result['in_cooldown'] is False
        assert result['current_workers'] == 2
        assert result['target_capacity'] > 2  # Scaled up
        # Should call update_auto_scaling_group
        mock_client.update_auto_scaling_group.assert_called_once()

    @patch('lambda_function.get_queue_depth')
    @patch('lambda_function.get_current_workers')
    @patch('lambda_function.is_in_cooldown')
    def test_scale_not_in_cooldown_scales_up(self, mock_is_cooldown, mock_get_workers, mock_get_queue):
        """Test scaling when not in cooldown and needs to scale up."""
        mock_conn = MagicMock()
        mock_client = MagicMock()

        mock_is_cooldown.return_value = False
        mock_get_workers.return_value = 2
        mock_get_queue.return_value = 20
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': [{
                'MinSize': 1,
                'MaxSize': 10
            }]
        }

        result = scale(mock_conn, mock_client, 'test-asg')

        assert result['scaled'] is True
        assert result['in_cooldown'] is False
        assert result['current_workers'] == 2
        assert result['target_capacity'] > 2
        mock_client.update_auto_scaling_group.assert_called_once()

    @patch('lambda_function.get_queue_depth')
    @patch('lambda_function.get_current_workers')
    @patch('lambda_function.is_in_cooldown')
    def test_scale_not_in_cooldown_already_at_capacity(self, mock_is_cooldown, mock_get_workers, mock_get_queue):
        """Test scaling when not in cooldown but already at target capacity."""
        mock_conn = MagicMock()
        mock_client = MagicMock()

        mock_is_cooldown.return_value = False
        mock_get_workers.return_value = 1
        mock_get_queue.return_value = 5
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': [{
                'MinSize': 1,
                'MaxSize': 10
            }]
        }

        result = scale(mock_conn, mock_client, 'test-asg')

        assert result['scaled'] is False
        assert result['in_cooldown'] is False
        assert result['current_workers'] == 1
        assert result['target_capacity'] == 1
        # Should not call update_auto_scaling_group
        mock_client.update_auto_scaling_group.assert_not_called()

    @patch('lambda_function.get_queue_depth')
    @patch('lambda_function.get_current_workers')
    @patch('lambda_function.is_in_cooldown')
    def test_scale_respects_min_max_bounds(self, mock_is_cooldown, mock_get_workers, mock_get_queue):
        """Test that scaling respects min/max bounds."""
        mock_conn = MagicMock()
        mock_client = MagicMock()

        mock_is_cooldown.return_value = False
        mock_get_workers.return_value = 2
        mock_get_queue.return_value = 10000
        mock_client.describe_auto_scaling_groups.return_value = {
            'AutoScalingGroups': [{
                'MinSize': 1,
                'MaxSize': 5
            }]
        }

        result = scale(mock_conn, mock_client, 'test-asg')

        assert result['target_capacity'] == 5  # max
        assert result['target_capacity'] <= 5
        assert result['target_capacity'] == 5


class TestLambdaHandler:
    @patch('lambda_function.scale')
    @patch('lambda_function.boto3')
    @patch('lambda_function.psycopg2')
    @patch.dict('os.environ', {
        'DATABASE_URL': 'postgresql://test',
        'AUTO_SCALING_GROUP_NAME': 'test-asg',
        'AWS_REGION': 'us-west-2'
    })
    def test_lambda_handler_success(self, mock_psycopg2, mock_boto3, mock_scale):
        """Test successful lambda handler execution."""
        mock_conn = MagicMock()
        mock_psycopg2.connect.return_value = mock_conn
        mock_scale.return_value = {
            'asg_name': 'test-asg',
            'scaled': True,
            'in_cooldown': False
        }

        event = {}
        context = MagicMock()

        result = lambda_handler(event, context)

        assert result['statusCode'] == 200
        body = json.loads(result['body'])
        assert body['scaled'] is True
        mock_conn.close.assert_called_once()

    @patch('lambda_function.psycopg2')
    @patch.dict('os.environ', {}, clear=True)
    def test_lambda_handler_missing_database_url(self, mock_psycopg2):
        """Test lambda handler with missing DATABASE_URL."""
        event = {}
        context = MagicMock()

        result = lambda_handler(event, context)

        assert result['statusCode'] == 500
        body = json.loads(result['body'])
        assert 'error' in body
        assert 'DATABASE_URL' in body['error']

    @patch('lambda_function.psycopg2')
    @patch.dict('os.environ', {
        'DATABASE_URL': 'postgresql://test'
    }, clear=True)
    def test_lambda_handler_missing_asg_name(self, mock_psycopg2):
        """Test lambda handler with missing AUTO_SCALING_GROUP_NAME."""
        event = {}
        context = MagicMock()

        result = lambda_handler(event, context)

        assert result['statusCode'] == 500
        body = json.loads(result['body'])
        assert 'error' in body
        assert 'AUTO_SCALING_GROUP_NAME' in body['error']

    @patch('lambda_function.scale')
    @patch('lambda_function.boto3')
    @patch('lambda_function.psycopg2')
    @patch.dict('os.environ', {
        'DATABASE_URL': 'postgresql://test',
        'AUTO_SCALING_GROUP_NAME': 'test-asg'
    })
    def test_lambda_handler_default_region(self, mock_psycopg2, mock_boto3, mock_scale):
        """Test lambda handler uses default region when not specified."""
        mock_conn = MagicMock()
        mock_psycopg2.connect.return_value = mock_conn
        mock_scale.return_value = {'scaled': False}

        event = {}
        context = MagicMock()

        lambda_handler(event, context)

        # Should use default region us-west-2
        mock_boto3.client.assert_called_with('autoscaling', region_name='us-west-2')

    @patch('lambda_function.scale')
    @patch('lambda_function.boto3')
    @patch('lambda_function.psycopg2')
    @patch.dict('os.environ', {
        'DATABASE_URL': 'postgresql://test',
        'AUTO_SCALING_GROUP_NAME': 'test-asg',
        'AWS_REGION': 'us-east-1'
    })
    def test_lambda_handler_custom_region(self, mock_psycopg2, mock_boto3, mock_scale):
        """Test lambda handler uses custom region when specified."""
        mock_conn = MagicMock()
        mock_psycopg2.connect.return_value = mock_conn
        mock_scale.return_value = {'scaled': False}

        event = {}
        context = MagicMock()

        lambda_handler(event, context)

        # Should use custom region
        mock_boto3.client.assert_called_with('autoscaling', region_name='us-east-1')

    @patch('lambda_function.scale')
    @patch('lambda_function.boto3')
    @patch('lambda_function.psycopg2')
    @patch.dict('os.environ', {
        'DATABASE_URL': 'postgresql://test',
        'AUTO_SCALING_GROUP_NAME': 'test-asg'
    })
    def test_lambda_handler_database_connection_retry(self, mock_psycopg2, mock_boto3, mock_scale):
        """Test lambda handler retries database connection on failure."""
        # Create a mock OperationalError class
        class MockOperationalError(Exception):
            pass

        mock_psycopg2.OperationalError = MockOperationalError
        mock_conn = MagicMock()

        # Fail first two attempts, succeed on third
        mock_psycopg2.connect.side_effect = [
            MockOperationalError("Connection failed"),
            MockOperationalError("Connection failed"),
            mock_conn
        ]
        mock_scale.return_value = {'scaled': False}

        event = {}
        context = MagicMock()

        result = lambda_handler(event, context)

        # Should have tried 3 times
        assert mock_psycopg2.connect.call_count == 3
        assert result['statusCode'] == 200
        mock_conn.close.assert_called_once()

