# Vector Role

This Ansible role installs and configures Vector for log shipping to AWS CloudWatch Logs and system metrics collection to AWS CloudWatch Metrics. By default, Vector is installed, enabled, and started.

## Requirements

- Ansible 2.9 or higher
- Ubuntu 22.04 or 24.04
- Root or sudo access
- AWS credentials configured (for CloudWatch Logs)

## Role Variables

### Installation

- `vector_install` (default: `true`): Whether to install Vector

### Service Configuration

- `vector_service_enabled` (default: `true`): Enable Vector service at boot
- `vector_service_state` (default: `started`): Service state (started/stopped)

### Configuration Paths

- `vector_config_dir` (default: `"/etc/vector"`): Vector configuration directory
- `vector_config_file` (default: `"/etc/vector/vector.yaml"`): Vector configuration file

### CloudWatch Configuration

- `vector_cloudwatch_log_group` (default: `"/boundless/bento/{{ hostvars[inventory_hostname]['ansible_hostname'] }}"`): CloudWatch Logs group name
  - **Important**: This must match an existing log group name in CloudWatch
  - For Pulumi-managed clusters, the pattern is typically `/boundless/bento/{{ stack_name }}/{{ component_type }}`
  - Example: `/boundless/bento/dev/manager` or `/boundless/bento/prod/prover`
- `vector_cloudwatch_region` (default: `"us-west-2"`): AWS region for CloudWatch Logs
- `vector_cloudwatch_stream_name` (default: `"%Y-%m-%d"`): CloudWatch Logs stream name pattern

### AWS Authentication

Vector supports multiple authentication methods for CloudWatch Logs. Credentials are resolved in this order:

1. **IAM Instance Profile** (default, preferred for EC2 instances)
   - `vector_aws_use_instance_profile` (default: `true`): Use IAM instance profile for authentication
   - No additional configuration needed if EC2 instance has IAM role attached

2. **AWS Credentials File**
   - `vector_aws_credentials_file` (default: `null`): Path to AWS credentials file (e.g., `"/root/.aws/credentials"`)
   - Vector will read credentials from the specified file

3. **Environment Variables**
   - `vector_aws_access_key_id` (default: `null`): AWS access key ID
   - `vector_aws_secret_access_key` (default: `null`): AWS secret access key
   - Credentials are set in `/etc/default/vector` environment file

### Service Monitoring

- `vector_monitor_services` (default: `["bento.service"]`): List of systemd units to monitor

### Buffer Configuration

- `vector_buffer_type` (default: `"memory"`): Buffer type (disk or memory)
- `vector_buffer_max_size` (default: `268435488`): Maximum buffer size in bytes (256MB)
- `vector_buffer_when_full` (default: `"block"`): Behavior when buffer is full (block or drop)

### Logging

- `vector_log_level` (default: `"info"`): Vector's own log level

### System Metrics Collection

- `vector_collect_host_metrics` (default: `true`): Enable collection of host metrics
- `vector_host_metrics_collectors` (default: `["memory", "disk", "filesystem"]`): List of metric collectors to enable
- `vector_metrics_scrape_interval` (default: `15`): Interval in seconds between metric scrapes
- `vector_cloudwatch_metrics_namespace` (default: `"Boundless/SystemMetrics"`): CloudWatch Metrics namespace for system metrics

With defaults, Vector collects and ships:

- **Memory**: total, free, used, cached, buffers
- **Disk**: I/O, space, inodes, read/write operations
- **Filesystem**: mount points, space, inodes

You can enable additional collectors (for example `cpu`, `load`, `network`,
`process`) by extending `vector_host_metrics_collectors`.

### Root volume alerting

When `vector_collect_host_metrics` is enabled and the `filesystem` collector is used (default), Vector sends **filesystem\_used\_ratio** (0–1) to CloudWatch Metrics under `vector_cloudwatch_metrics_namespace` (default `Boundless/SystemMetrics`). Root (/) is included by default via `vector_filesystem_mountpoints_includes: ["/"]`.

Infrastructure (e.g. Pulumi prover stacks) can create a CloudWatch alarm on this metric to alert when the root volume is filling up, for example:

- **Metric**: `filesystem_used_ratio`, namespace `Boundless/SystemMetrics`
- **Dimensions**: `mountpoint="/"`, `host=<instance short hostname, e.g. ip-10-0-0-5>`
- **Threshold**: e.g. ≥ 0.85 (85%) for 2 consecutive 5-minute periods

### Bento process and container tracking

When `vector_track_bento_process` is enabled (default: `true`), Vector runs scheduled checks and publishes gauges to CloudWatch Metrics:

- **bento\_active**: `1` when the `bento` systemd unit is active, `0` otherwise
- **bento\_containers**: Number of running Docker containers in the Bento Compose project (`com.docker.compose.project=bento`)

Checks run every `vector_bento_process_interval_secs` seconds (default: 60). Use these metrics for dashboards and alarms (e.g. bento\_active < 1 or bento\_containers below expected).

### Kailua process and staleness tracking

When `vector_track_kailua_process` is enabled (default: `false`), Vector runs scheduled checks for Kailua and publishes gauges to CloudWatch Metrics:

- **kailua\_active**: `1` when the `kailua` systemd unit is active, `0` otherwise
- **kailua\_log\_age\_secs**: Seconds since the last `kailua.service` log line. A high value (e.g. > 7200) means the prover has stalled.

Checks run every `vector_kailua_process_interval_secs` seconds (default: 60).

**Recommended CloudWatch Alarm** (stale prover detection):

- **Metric**: `kailua_log_age_secs`, namespace `Boundless/SystemMetrics`
- **Threshold**: `> 7200` (2 hours with no proof activity)
- **Period**: 300 seconds, 2 consecutive datapoints
- **Action**: SNS topic → email/Slack/PagerDuty

Also add `kailua.service` to `vector_monitor_services` so journald logs are shipped to CloudWatch Logs. Example host vars:

```yaml
vector_monitor_services:
  - bento.service
  - kailua.service
vector_track_kailua_process: true
```

### Log error metrics

When `vector_track_log_errors` is enabled (default: `true`), Vector counts journald log lines containing `"ERROR"` and publishes a counter to CloudWatch Metrics:

- **bento\_log\_errors**: Incremented by 1 for each ERROR line from the monitored units

Use this metric for error-rate dashboards and alarms (e.g. threshold on Sum over 5 minutes).

**If Vector fails to start** after a deploy (e.g. "Job for vector.service failed"), check `journalctl -xeu vector.service` for the config error. To get Vector running again you can temporarily disable these features in group/host vars: `vector_track_bento_process: false` and/or `vector_track_log_errors: false`. Bento tracking and log error metrics require Vector 0.28+ with exec source and log\_to\_metric support.

## Dependencies

- **AWS CLI**: Optional
  - The `vector` role itself does not require `aws` binary to run.
  - Run the `awscli` role first if your workflow creates/validates CloudWatch resources with CLI commands.

## Example Playbook

```yaml
---
- hosts: all
  become: true
  roles:
    # Install AWS CLI first (required for log group operations)
    - role: awscli
    # Install and configure Vector
    - role: vector
      vars:
        # Enable Vector service
        vector_service_enabled: true
        vector_service_state: started
        # Customize CloudWatch log group
        vector_cloudwatch_log_group: "/boundless/bento/production"
```

## What This Role Does

1. **Installs Vector** from the official Vector repository
2. **Creates configuration directory** `/etc/vector`
3. **Configures Vector** to:
   - Monitor systemd journald logs for specified services
   - Extract log messages
   - Ship logs to AWS CloudWatch Logs
   - Collect system metrics (CPU, memory, disk, network) when enabled
   - Ship metrics to AWS CloudWatch Metrics when enabled
4. **Configures Vector environment** (`/etc/default/vector`) with AWS credentials (if provided)
5. **Manages Vector service** (enabled and started by default)

**Note**: This role does not install AWS CLI. If you need to create CloudWatch log groups automatically, ensure the `awscli` role runs before this role.

## Metrics Collection

When `vector_collect_host_metrics` is enabled (default: `true`), Vector automatically collects system metrics and publishes them to CloudWatch Metrics under the namespace specified by `vector_cloudwatch_metrics_namespace`.

Metrics are collected every `vector_metrics_scrape_interval` seconds (default:
15s). With default collectors, this includes memory, disk, and filesystem
metrics. Additional categories are available by enabling more collectors.

You can view these metrics in the AWS CloudWatch console under **Metrics** → **Boundless/SystemMetrics** (or your custom namespace).

## Service Management

Vector is enabled and started by default. To disable or stop it, override these variables:

```yaml
vector_service_enabled: false
vector_service_state: stopped
```

Or override when running the playbook:

```bash
ansible-playbook -i inventory.yml monitoring.yml -e "vector_service_enabled=false" -e "vector_service_state=stopped"
```

## AWS Credentials

Vector requires AWS credentials to ship logs to CloudWatch. The role supports multiple authentication methods:

### Recommended: IAM Instance Profile (EC2)

For EC2 instances, attach an IAM role with CloudWatch Logs permissions:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "logs:CreateLogGroup",
        "logs:CreateLogStream",
        "logs:DescribeLogGroups",
        "logs:DescribeLogStreams",
        "logs:PutLogEvents"
      ],
      "Resource": "*"
    }
  ]
}
```

No additional configuration needed - Vector will automatically use the instance profile.

### Alternative: AWS Credentials File

```yaml
vector_aws_credentials_file: "/root/.aws/credentials"
```

Ensure the credentials file exists and has the format:

```ini
[default]
aws_access_key_id = YOUR_ACCESS_KEY
aws_secret_access_key = YOUR_SECRET_KEY
```

### Alternative: Environment Variables

```yaml
vector_aws_access_key_id: "AKIAIOSFODNN7EXAMPLE"
vector_aws_secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
```

**Note**: Storing credentials in variables is less secure. Prefer IAM roles or credentials files.

## Tags

- `vector` - All vector-related tasks
- `vector-config` - Configuration tasks
- `vector-service` - Service management

## License

BSD/MIT

## Author Information

Boundless
