import os
import json
import math
import logging
from typing import Dict, Any, Optional
from datetime import datetime, timezone, timedelta

import boto3
import psycopg2
from psycopg2.extras import RealDictCursor

logger = logging.getLogger()
logger.setLevel(logging.INFO)

# Cooldown period: 30 minutes
COOLDOWN_MINUTES = 30


def get_queue_depth(conn) -> int:
    with conn.cursor(cursor_factory=RealDictCursor) as cur:
        cur.execute("""
            SELECT COUNT(*) as count FROM
                tasks
                JOIN jobs ON tasks.job_id = jobs.id
                CROSS JOIN LATERAL jsonb_object_keys(tasks.task_def) task_type
            WHERE (
                tasks.state = 'pending' OR tasks.state = 'ready'
            ) AND (
                task_type = 'Join' OR task_type = 'Prove' OR task_type = 'Resolve'
            ) AND (
                jobs.state = 'running'
            )
        """)
        result = cur.fetchone()
        return result['count'] if result else 0


def required_workers(queue_depth: int) -> int:
    """Calculate required workers using log2 rounding up."""
    if queue_depth <= 5:
        return 1
    return int(math.ceil(math.log2(queue_depth)) * 2)


def get_current_workers(autoscaling_client, asg_name: str) -> int:
    """Get the current desired capacity of the ASG."""
    response = autoscaling_client.describe_auto_scaling_groups(
        AutoScalingGroupNames=[asg_name]
    )

    if not response['AutoScalingGroups']:
        raise ValueError(f"Auto scaling group '{asg_name}' not found")

    group = response['AutoScalingGroups'][0]
    desired_capacity = group.get('DesiredCapacity')

    if desired_capacity is None:
        raise ValueError(f"Desired capacity not set for ASG '{asg_name}'")

    return desired_capacity


def get_last_scaling_time(autoscaling_client, asg_name: str) -> Optional[datetime]:
    """Get the start time of the most recent scaling activity."""
    try:
        response = autoscaling_client.describe_scaling_activities(
            AutoScalingGroupName=asg_name,
            MaxRecords=1
        )

        activities = response.get('Activities', [])
        if not activities:
            return None

        # Get the most recent activity (first in the list)
        activity = activities[0]
        start_time = activity.get('StartTime')

        if start_time:
            # AWS SDK returns datetime objects, ensure it's timezone-aware
            if isinstance(start_time, datetime):
                # If it's naive, assume UTC
                if start_time.tzinfo is None:
                    return start_time.replace(tzinfo=timezone.utc)
                return start_time
            # If it's a string, parse it
            elif isinstance(start_time, str):
                return datetime.fromisoformat(start_time.replace('Z', '+00:00'))

        return None
    except Exception as e:
        logger.warning(f"Failed to get last scaling time: {e}")
        return None


def is_in_cooldown(autoscaling_client, asg_name: str) -> bool:
    """Check if the ASG is still in cooldown period (10 minutes since last scaling)."""
    last_scaling_time = get_last_scaling_time(autoscaling_client, asg_name)

    if last_scaling_time is None:
        # No previous scaling activity, cooldown doesn't apply
        return False

    now = datetime.now(timezone.utc)
    time_since_last_scale = now - last_scaling_time
    cooldown_period = timedelta(minutes=COOLDOWN_MINUTES)

    return time_since_last_scale < cooldown_period


def scale(
    conn,
    autoscaling_client,
    asg_name: str
) -> Dict[str, Any]:
    # Get queue depth and calculate required workers
    queue_depth = get_queue_depth(conn)
    required_workers_count = required_workers(queue_depth)

    # Get current ASG state
    current_workers = get_current_workers(autoscaling_client, asg_name)

    # Get ASG constraints
    response = autoscaling_client.describe_auto_scaling_groups(
        AutoScalingGroupNames=[asg_name]
    )
    group = response['AutoScalingGroups'][0]
    min_size = group.get('MinSize', 0)
    max_size = group.get('MaxSize')

    if max_size is None:
        raise ValueError(f"Max size not set for ASG '{asg_name}'")

    # Clamp required workers to min/max bounds
    target_capacity = max(min_size, min(required_workers_count, max_size))

    # Check if we're scaling down (cooldown only applies to scale down)
    is_scaling_down = target_capacity < current_workers

    # Apply cooldown only when scaling down
    if is_scaling_down and is_in_cooldown(autoscaling_client, asg_name):
        last_scaling_time = get_last_scaling_time(autoscaling_client, asg_name)
        now = datetime.now(timezone.utc)
        time_since_last_scale = now - last_scaling_time if last_scaling_time else timedelta(0)
        remaining_cooldown = timedelta(minutes=COOLDOWN_MINUTES) - time_since_last_scale

        logger.info(
            f"ASG {asg_name} is in cooldown period (scale down blocked). "
            f"Would scale from {current_workers} to {target_capacity} workers, "
            f"but last scaling: {last_scaling_time}, "
            f"remaining cooldown: {remaining_cooldown.total_seconds() / 60:.1f} minutes"
        )

        return {
            'asg_name': asg_name,
            'queue_depth': queue_depth,
            'required_workers': required_workers_count,
            'current_workers': current_workers,
            'target_capacity': current_workers,  # Keep current capacity
            'min_size': min_size,
            'max_size': max_size,
            'scaled': False,
            'in_cooldown': True,
            'last_scaling_time': last_scaling_time.isoformat() if last_scaling_time else None,
            'remaining_cooldown_seconds': int(remaining_cooldown.total_seconds()) if remaining_cooldown.total_seconds() > 0 else 0
        }

    result = {
        'asg_name': asg_name,
        'queue_depth': queue_depth,
        'required_workers': required_workers_count,
        'current_workers': current_workers,
        'target_capacity': target_capacity,
        'min_size': min_size,
        'max_size': max_size,
        'scaled': False,
        'in_cooldown': False
    }

    if current_workers != target_capacity:
        logger.info(
            f"Scaling ASG {asg_name} from {current_workers} to {target_capacity} workers "
            f"(required: {required_workers_count}, min: {min_size}, max: {max_size})"
        )

        autoscaling_client.update_auto_scaling_group(
            AutoScalingGroupName=asg_name,
            DesiredCapacity=target_capacity
        )

        result['scaled'] = True
    else:
        logger.info(
            f"ASG {asg_name} already at target capacity: {current_workers} workers"
        )

    return result


def lambda_handler(event: Dict[str, Any], context: Any) -> Dict[str, Any]:
    """Lambda handler entry point."""
    try:
        # Get configuration from environment variables
        database_url = os.environ.get('DATABASE_URL')
        if not database_url:
            raise ValueError("DATABASE_URL environment variable is required")

        asg_name = os.environ.get('AUTO_SCALING_GROUP_NAME')
        if not asg_name:
            raise ValueError("AUTO_SCALING_GROUP_NAME environment variable is required")

        aws_region = os.environ.get('AWS_REGION', 'us-west-2')

        # Create database connection with timeout and retry
        # Lambda in VPC may need a moment to establish network connectivity
        import time
        max_retries = 3
        retry_delay = 2

        conn = None
        for attempt in range(max_retries):
            try:
                conn = psycopg2.connect(
                    database_url,
                    connect_timeout=10
                )
                break
            except psycopg2.OperationalError as e:
                if attempt < max_retries - 1:
                    logger.warning(f"Connection attempt {attempt + 1} failed: {e}. Retrying in {retry_delay} seconds...")
                    time.sleep(retry_delay)
                else:
                    raise

        try:
            # Create AWS Auto Scaling client
            autoscaling_client = boto3.client('autoscaling', region_name=aws_region)

            # Run scaling logic
            result = scale(conn, autoscaling_client, asg_name)

            return {
                'statusCode': 200,
                'body': json.dumps(result)
            }
        finally:
            conn.close()

    except Exception as e:
        logger.error(f"Error scaling ASG: {str(e)}", exc_info=True)
        return {
            'statusCode': 500,
            'body': json.dumps({
                'error': str(e)
            })
        }

