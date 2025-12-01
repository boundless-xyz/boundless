import os
import json
import math
import logging
from typing import Dict, Any

import boto3
import psycopg2
from psycopg2.extras import RealDictCursor

logger = logging.getLogger()
logger.setLevel(logging.INFO)


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

    result = {
        'asg_name': asg_name,
        'queue_depth': queue_depth,
        'required_workers': required_workers_count,
        'current_workers': current_workers,
        'target_capacity': target_capacity,
        'min_size': min_size,
        'max_size': max_size,
        'scaled': False
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

