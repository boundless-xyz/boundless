# Vector Role

This Ansible role installs and configures Vector for log shipping to AWS CloudWatch Logs and system metrics collection to AWS CloudWatch Metrics. By default, Vector is installed, enabled, and started.

## Requirements

* Ansible 2.9 or higher
* Ubuntu 22.04 or 24.04
* Root or sudo access
* AWS credentials configured (for CloudWatch Logs)

## Role Variables

### Installation

* `vector_install` (default: `true`): Whether to install Vector

### Service Configuration

* `vector_service_enabled` (default: `true`): Enable Vector service at boot
* `vector_service_state` (default: `started`): Service state (started/stopped)

### Configuration Paths

* `vector_config_dir` (default: `"/etc/vector"`): Vector configuration directory
* `vector_config_file` (default: `"/etc/vector/vector.yaml"`): Vector configuration file

### CloudWatch Configuration

* `vector_cloudwatch_log_group` (default: `"/boundless/bento/{{ hostvars[inventory_hostname]['ansible_hostname'] }}"`): CloudWatch Logs group name
  * **Important**: This must match an existing log group name in CloudWatch
  * For Pulumi-managed clusters, the pattern is typically `/boundless/bento/{{ stack_name }}/{{ component_type }}`
  * Example: `/boundless/bento/dev/manager` or `/boundless/bento/prod/prover`
* `vector_cloudwatch_region` (default: `"us-west-2"`): AWS region for CloudWatch Logs
* `vector_cloudwatch_stream_name` (default: `"%Y-%m-%d"`): CloudWatch Logs stream name pattern

### AWS Authentication

Vector supports multiple authentication methods for CloudWatch Logs. Credentials are resolved in this order:

1. **IAM Instance Profile** (default, preferred for EC2 instances)
   * `vector_aws_use_instance_profile` (default: `true`): Use IAM instance profile for authentication
   * No additional configuration needed if EC2 instance has IAM role attached

2. **AWS Credentials File**
   * `vector_aws_credentials_file` (default: `null`): Path to AWS credentials file (e.g., `"/root/.aws/credentials"`)
   * Vector will read credentials from the specified file

3. **Environment Variables**
   * `vector_aws_access_key_id` (default: `null`): AWS access key ID
   * `vector_aws_secret_access_key` (default: `null`): AWS secret access key
   * Credentials are set in `/etc/default/vector` environment file

### Service Monitoring

* `vector_monitor_services` (default: `["bento-api.service", "bento-exec.service", "bento-aux.service", "bento-prove.service", "broker.service", "miner.service"]`): List of systemd units to monitor

### Buffer Configuration

* `vector_buffer_type` (default: `"memory"`): Buffer type (disk or memory)
* `vector_buffer_max_size` (default: `268435488`): Maximum buffer size in bytes (256MB)
* `vector_buffer_when_full` (default: `"block"`): Behavior when buffer is full (block or drop)

### Logging

* `vector_log_level` (default: `"info"`): Vector's own log level

### System Metrics Collection

* `vector_collect_host_metrics` (default: `true`): Enable collection of system metrics (CPU, memory, disk, network)
* `vector_host_metrics_collectors` (default: `["cpu", "memory", "disk", "filesystem", "load", "network", "process"]`): List of metric collectors to enable
* `vector_metrics_scrape_interval` (default: `15`): Interval in seconds between metric scrapes
* `vector_cloudwatch_metrics_namespace` (default: `"Boundless/SystemMetrics"`): CloudWatch Metrics namespace for system metrics

When enabled, Vector collects and ships the following system metrics to CloudWatch Metrics:

* **CPU**: usage, frequency, temperature
* **Memory**: total, free, used, cached, buffers
* **Disk**: I/O, space, inodes, read/write operations
* **Network**: bytes, packets, errors, drops
* **System**: load average, uptime, processes
* **Filesystem**: mount points, space, inodes

### Root volume alerting

When `vector_collect_host_metrics` is enabled and the `filesystem` collector is used (default), Vector sends **filesystem\_used\_ratio** (0–1) to CloudWatch Metrics under `vector_cloudwatch_metrics_namespace` (default `Boundless/SystemMetrics`). Root (/) is included by default via `vector_filesystem_mountpoints_includes: ["/"]`.

Infrastructure (e.g. Pulumi prover stacks) can create a CloudWatch alarm on this metric to alert when the root volume is filling up, for example:

* **Metric**: `filesystem_used_ratio`, namespace `Boundless/SystemMetrics`
* **Dimensions**: `mountpoint="/"`, `host=<instance short hostname, e.g. ip-10-0-0-5>`
* **Threshold**: e.g. ≥ 0.85 (85%) for 2 consecutive 5-minute periods

## Dependencies

* **AWS CLI**: Required
  * The `awscli` role should be run before the `vector` role if log group creation is needed
  * AWS CLI is automatically installed by the `monitoring.yml` playbook

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
   * Monitor systemd journald logs for specified services
   * Extract log messages
   * Ship logs to AWS CloudWatch Logs
   * Collect system metrics (CPU, memory, disk, network) when enabled
   * Ship metrics to AWS CloudWatch Metrics when enabled
4. **Configures Vector environment** (`/etc/default/vector`) with AWS credentials (if provided)
5. **Manages Vector service** (enabled and started by default)

**Note**: This role does not install AWS CLI. If you need to create CloudWatch log groups automatically, ensure the `awscli` role runs before this role.

## Metrics Collection

When `vector_collect_host_metrics` is enabled (default: `true`), Vector automatically collects system metrics and publishes them to CloudWatch Metrics under the namespace specified by `vector_cloudwatch_metrics_namespace`.

Metrics are collected every `vector_metrics_scrape_interval` seconds (default: 15s) and include:

* CPU utilization and load average
* Memory usage (total, free, used, cached)
* Disk I/O and filesystem usage
* Network traffic and errors
* System uptime and process counts

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

* `vector` - All vector-related tasks
* `vector-config` - Configuration tasks
* `vector-service` - Service management

## License

BSD/MIT

## Author Information

Boundless
