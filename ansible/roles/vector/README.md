# Vector Role

This Ansible role installs and configures Vector for log shipping to AWS CloudWatch Logs. By default, Vector is installed but **disabled** (service stopped and not enabled at boot).

## Requirements

* Ansible 2.9 or higher
* Ubuntu 22.04 or 24.04
* Root or sudo access
* AWS credentials configured (for CloudWatch Logs)

## Role Variables

### Installation

* `vector_install` (default: `true`): Whether to install Vector

### Service Configuration

* `vector_service_enabled` (default: `false`): Enable Vector service at boot (**disabled by default**)
* `vector_service_state` (default: `stopped`): Service state (**stopped by default**)

### Configuration Paths

* `vector_config_dir` (default: `"/etc/vector"`): Vector configuration directory
* `vector_config_file` (default: `"/etc/vector/vector.yaml"`): Vector configuration file

### CloudWatch Configuration

* `vector_cloudwatch_log_group` (default: `"/boundless/bento"`): CloudWatch Logs group name
* `vector_cloudwatch_region` (default: `"us-west-2"`): AWS region for CloudWatch Logs
* `vector_cloudwatch_stream_name` (default: `"%Y-%m-%d"`): CloudWatch Logs stream name pattern

### Service Monitoring

* `vector_monitor_services` (default: `["bento.service", "broker.service", "miner.service"]`): List of systemd units to monitor

### Buffer Configuration

* `vector_buffer_type` (default: `"disk"`): Buffer type (disk or memory)
* `vector_buffer_max_size` (default: `268435488`): Maximum buffer size in bytes (256MB)
* `vector_buffer_when_full` (default: `"block"`): Behavior when buffer is full (block or drop)

### Logging

* `vector_log_level` (default: `"info"`): Vector's own log level

## Dependencies

None.

## Example Playbook

```yaml
---
- hosts: all
  become: true
  roles:
    - role: vector
      vars:
        # Enable Vector service
        vector_service_enabled: true
        vector_service_state: started
        # Customize CloudWatch log group
        vector_cloudwatch_log_group: "/boundless/bento/production"
```

## What This Role Does

1. **Installs Vector** from the official Timber.io repository
2. **Creates configuration directory** `/etc/vector`
3. **Configures Vector** to:
   * Monitor systemd journald logs for specified services
   * Extract log messages
   * Ship logs to AWS CloudWatch Logs
4. **Configures Vector environment** (`/etc/default/vector`)
5. **Manages Vector service** (disabled and stopped by default)

## Enabling Vector

To enable Vector log shipping, set these variables:

```yaml
vector_service_enabled: true
vector_service_state: started
```

Or override when running the playbook:

```bash
ansible-playbook -i inventory.yml cluster.yml -e "vector_service_enabled=true" -e "vector_service_state=started"
```

## AWS Credentials

Vector requires AWS credentials to ship logs to CloudWatch. Ensure one of the following is configured:

* IAM instance profile (for EC2 instances)
* AWS credentials file (`~/.aws/credentials`)
* Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)

## Tags

* `vector` - All vector-related tasks
* `vector-config` - Configuration tasks
* `vector-service` - Service management

## License

BSD/MIT

## Author Information

Boundless
