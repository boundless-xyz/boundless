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
* `broker_user` (default: `"ubuntu"`): User to run the service as
* `broker_install_dir` (default: `"/usr/local/bin"`): Directory to install binary
* `broker_config_dir` (default: `"/opt/boundless"`): Directory for configuration files
* `broker_work_dir` (default: `"/opt/boundless"`): Working directory for the service

#### Service Configuration

* `broker_service_enabled` (default: `true`): Enable service to start on boot
* `broker_service_state` (default: `started`): Service state (started, stopped, restarted)
* `broker_timeout_start_sec` (default: `1800`): Service start timeout in seconds
* `broker_timeout_stop_sec` (default: `300`): Service stop timeout in seconds

#### Bento API Configuration

* `broker_bento_api_url` (default: `"http://localhost:8080"`): URL of the Bento REST API

#### Broker Market Configuration

* `broker_min_mcycle_price` (default: `"0.00000001"`): Minimum price per mega-cycle to lock orders
* `broker_min_mcycle_price_collateral_token` (default: `"0.00005"`): Minimum price per mega-cycle in collateral token
* `broker_peak_prove_khz` (default: `100`): Estimated peak performance in kHz
* `broker_max_collateral` (default: `"200"`): Maximum collateral amount in ZKC
* `broker_max_concurrent_preflights` (default: `2`): Maximum concurrent preflight tasks
* `broker_max_concurrent_proofs` (default: `1`): Maximum concurrent proof tasks
* `broker_priority_requestor_lists` (default: list with Boundless recommended list): List of priority requestor list URLs

## Dependencies

None

## Example Playbook

### Basic SQLite Configuration

```yaml
---
- hosts: brokers
  become: true
  roles:
    - role: broker
      vars:
        broker_version: "v1.2.0"
        broker_bento_api_url: "http://localhost:8080"
        broker_db_type: "sqlite"
        broker_db_url: "sqlite:///opt/boundless/broker.db"
```

### PostgreSQL Configuration

```yaml
---
- hosts: brokers
  become: true
  roles:
    - role: broker
      vars:
        broker_version: "v1.2.0"
        broker_bento_api_url: "http://bento-api.example.com:8080"
        broker_peak_prove_khz: 200
        broker_max_concurrent_proofs: 4
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
* **Database Support**: Supports both SQLite and PostgreSQL databases
* **Directory Management**: Creates necessary directories with proper permissions
* **Clean Installation**: Downloads, installs binary, and cleans up temporary files

## Handlers

* `Reload systemd`: Reloads systemd daemon after service file changes
* `Restart Broker service`: Restarts the Broker service
* `Start Broker service`: Starts the Broker service
* `Stop Broker service`: Stops the Broker service

## Important Notes

1. **Configuration Warning**: The default broker.toml template includes a disclaimer that these settings may not be optimal for production. It's strongly recommended to use the Boundless CLI's `boundless prover generate-config` command to generate a tailored configuration.

2. **Bento API Dependency**: The broker requires the Bento REST API to be running and accessible at the configured URL.

3. **Database Choice**:
   * SQLite is simpler for single-instance deployments
   * PostgreSQL is recommended for production or multi-instance deployments

4. **Performance Tuning**: Adjust `broker_peak_prove_khz`, `broker_max_concurrent_preflights`, and `broker_max_concurrent_proofs` based on your hardware capabilities and benchmarking results.

## License

BSD/MIT

## Author Information

Boundless
