# Miner Role

This Ansible role installs and configures the Boundless Miner service, which continuously mines for rewards using the Bento CLI.

## Requirements

* Ansible 2.9 or higher
* Ubuntu 22.04 or 24.04
* Root or sudo access
* Bento API service running (for miner to connect to)

## Role Variables

### Installation

* `miner_version` (default: `{{ bento_version | default('v1.1.1') }}`): Version reference (not used for installation)

### Service Configuration

* `miner_service_enabled` (default: `true`): Enable Miner service at boot
* `miner_service_state` (default: `started`): Service state (started, stopped, restarted)

### User Configuration

* `miner_user` (default: `"{{ bento_user | default('bento') }}"`): System user for Miner service
* `miner_group` (default: `"{{ bento_group | default('bento') }}"`): System group for Miner service
* `miner_home` (default: `"{{ bento_home | default('/var/lib/bento') }}"`): Home directory for Miner user

### Mining Configuration

* `miner_iter_count` (default: `500000`): Number of iterations per mining cycle
* `miner_sleep_seconds` (default: `1`): Sleep duration between mining cycles (in seconds)

### Service Timeouts

* `miner_timeout_start_sec` (default: `60`): Service start timeout
* `miner_timeout_stop_sec` (default: `30`): Service stop timeout

### Installation Paths

* `miner_install_dir` (default: `"/usr/local/bin"`): Installation directory for binary

### Environment Configuration

The miner role does not create an environment file. If `bento-cli` requires
custom environment variables, add them via a systemd drop-in or an
`EnvironmentFile` you manage yourself.

## Dependencies

* Requires Bento API service to be running
* Uses same database, Redis, and S3 configuration as Bento services

## Example Playbook

```yaml
---
- hosts: miners
  become: true
  roles:
    - role: miner
      vars:
        miner_iter_count: 1000000
        miner_sleep_seconds: 2
```

## What This Role Does

1. **Checks for bento-cli** in `miner_install_dir`
2. **Creates systemd service** that runs miner in a continuous loop
3. **Starts and enables** the miner service

## Service Behavior

The miner service runs `bento-cli --iter-count <count>` in a continuous loop with a configurable sleep interval between cycles. If the mining process exits, systemd will automatically restart it.

## Tags

* `miner` - All miner-related tasks
* `miner-install` - Binary installation
* `miner-config` - Configuration tasks
* `miner-service` - Service management

## License

BSD/MIT

## Author Information

Boundless
