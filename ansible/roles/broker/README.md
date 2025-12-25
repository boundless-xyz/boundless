# Broker Role

This role installs and configures the Boundless Broker service for managing proof orders.

## Requirements

* Ansible 2.9 or later
* Ubuntu 22.04 or 24.04
* System with systemd
* Sudo/root privileges
* Bento API service running (for broker to connect to)

## Role Variables

### Required Variables

None - all variables have defaults, but you should override them based on your environment.

### Optional Variables

#### Installation Configuration

* `broker_version` (default: `"v1.1.1"`): Version of Broker to install
* `broker_user` (default: `"{{ bento_user | default('bento') }}"`): User to run the service as (defaults to `bento` user)
* `broker_group` (default: `"{{ bento_group | default('bento') }}"`): Group for the broker user (defaults to `bento` group)
* `broker_install_dir` (default: `"/usr/local/bin"`): Directory to install binary
* `broker_config_dir` (default: `"/etc/boundless/broker"`): Directory for broker-specific configuration files
* `broker_work_dir` (default: `"/etc/boundless/broker"`): Working directory for the service

#### Service Configuration

* `broker_service_enabled` (default: `true`): Enable service to start on boot
* `broker_service_state` (default: `started`): Service state (started, stopped, restarted)
* `broker_timeout_start_sec` (default: `1800`): Service start timeout in seconds
* `broker_timeout_stop_sec` (default: `300`): Service stop timeout in seconds

#### Bento API Configuration

* `broker_bento_api_url` (default: `"http://localhost:8081"`): URL of the Bento REST API

#### Broker Market Configuration

* `broker_min_mcycle_price` (default: `"0.0000003"`): Minimum price per mega-cycle
* `broker_min_mcycle_price_collateral_token` (default: `"0"`): Minimum price per mega-cycle in collateral token
* `broker_peak_prove_khz` (default: `100`): Estimated peak performance in kHz
* `broker_max_collateral` (default: `"200"`): Maximum collateral amount in ZKC
* `broker_max_concurrent_preflights` (default: `4`): Maximum concurrent preflight tasks (auto-calculated from CPU count if not set)
* `broker_max_concurrent_proofs` (default: `1`): Maximum concurrent proof tasks
* `broker_priority_requestor_lists` (default: list with Boundless recommended list): List of priority requestor list URLs
* `broker_prometheus_metrics_addr` (default: `"127.0.0.1:9090"`): Prometheus metrics endpoint

#### Ethereum Configuration

* `broker_rpc_url` (default: `"https://rpc.boundless.network"`): Ethereum RPC URL (**should be overridden**)
* `broker_private_key` (default: zero key): Private key for signing transactions (**must be set**)

## Dependencies

None

## Example Playbook

### Basic Configuration

```yaml
---
- hosts: brokers
  become: true
  roles:
    - role: broker
      vars:
        broker_version: "v1.2.0"
        broker_bento_api_url: "http://localhost:8080"
        broker_db_url: "sqlite:///opt/boundless/broker.db"
```

### Custom Market Configuration

```yaml
---
- hosts: brokers
  become: true
  roles:
    - role: broker
      vars:
        broker_min_mcycle_price: "0.00000002"
        broker_min_mcycle_price_collateral_token: "0.0001"
        broker_peak_prove_khz: 500
        broker_max_collateral: "500"
        broker_max_concurrent_preflights: 4
        broker_max_concurrent_proofs: 2
        broker_priority_requestor_lists:
          - "https://requestors.boundless.network/boundless-recommended-priority-list.standard.json"
          - "https://example.com/custom-priority-list.json"
```

## Features

* **Idempotent**: Safe to run multiple times
* **Service Management**: Properly manages systemd service with handlers
* **Configuration Management**: Creates broker.toml configuration file from template
* **Database Support**: Uses SQLite database (the only supported database type)
* **Directory Management**: Creates necessary directories with proper permissions (shared `/etc/boundless/` directory)
* **Ansible Variable System**: All configuration variables follow Ansible's standard variable precedence (host\_vars → group\_vars → defaults)
* **Prometheus Metrics**: Configurable Prometheus metrics endpoint (defaults to `127.0.0.1:9090`)
* **Clean Installation**: Downloads, installs binary, and cleans up temporary files
* **Shared User**: Uses the `bento` user by default (same user as Bento services) to simplify permissions and configuration

## Handlers

* `Reload systemd`: Reloads systemd daemon after service file changes
* `Restart Broker service`: Restarts the Broker service
* `Start Broker service`: Starts the Broker service
* `Stop Broker service`: Stops the Broker service

## Important Notes

1. **Configuration Warning**: The default broker.toml template includes a disclaimer that these settings may not be optimal for production. It's strongly recommended to use the Boundless CLI's `boundless prover generate-config` command to generate a tailored configuration.

2. **Bento API Dependency**: The broker requires the Bento REST API to be running and accessible at the configured URL.

3. **Database**: Broker only supports SQLite. The database file is stored at the location specified in `broker_db_url`.

4. **Performance Tuning**: Adjust `broker_peak_prove_khz`, `broker_max_concurrent_preflights`, and `broker_max_concurrent_proofs` based on your hardware capabilities and benchmarking results. `broker_max_concurrent_preflights` is automatically calculated from CPU count if not set.

5. **Shared Directory**: Broker uses `/etc/boundless/broker/` for its configuration and database files, while sharing the parent `/etc/boundless/` directory with Bento services. The parent directory must have `0755` permissions and be owned by `root:root` to allow both services to access it. The Bento role automatically sets these permissions to prevent conflicts.

6. **Private Key**: The `broker_private_key` must be set (not the default zero key). Set it in `host_vars/HOSTNAME/vault.yml` or via command-line (`-e broker_private_key="0x..."`).

7. **User Configuration**: By default, the broker service runs as the `bento` user (same as Bento services) to simplify permissions. This can be overridden by setting `broker_user` and `broker_group` variables.

## Troubleshooting

### Service Not Starting

Check service status and logs:

```bash
systemctl status broker
journalctl -u broker -f
journalctl -u broker -n 100
```

### Permission Issues

If broker can't access `/etc/boundless/`:

```bash
# Check directory permissions
ls -ld /etc/boundless/
# Should be: drwxr-xr-x root root
# Fix: sudo chmod 0755 /etc/boundless && sudo chown root:root /etc/boundless
```

### Configuration Issues

Verify broker configuration:

```bash
cat /etc/boundless/broker.toml
# Check that all required values are set (not defaults)
```

### Database Issues

Check SQLite database:

```bash
ls -l /etc/boundless/broker.db
# Should be readable/writable by broker user
```

## License

BSD/MIT

## Author Information

Boundless
