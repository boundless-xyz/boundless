# Bento Role

This role installs and configures the Bento agent service for Boundless provers.

## Requirements

* Ansible 2.9 or later
* Ubuntu 22.04 or 24.04
* System with systemd
* Sudo/root privileges

## Role Variables

### Required Variables

None - all variables have defaults, but you should override them based on your environment.

### Optional Variables

#### Installation Configuration

* `bento_version` (default: `"v1.1.0"`): Version of Bento to install
* `bento_task` (default: `"prove"`): Task type for the agent (prove, execute, aux, etc.)
* `bento_user` (default: `"ubuntu"`): User to run the service as
* `bento_install_dir` (default: `"/usr/local/bin"`): Directory to install binaries
* `bento_config_dir` (default: `"/etc/boundless"`): Directory for configuration files
* `bento_work_dir` (default: `"/opt/boundless"`): Working directory for the service

#### Service Configuration

* `bento_service_enabled` (default: `true`): Enable service to start on boot
* `bento_service_state` (default: `started`): Service state (started, stopped, restarted)
* `bento_agent_timeout_start_sec` (default: `1800`): Service start timeout in seconds
* `bento_agent_timeout_stop_sec` (default: `300`): Service stop timeout in seconds

#### Environment Variables

* `bento_segment_po2` (default: `20`): Segment size (number of r0vm cycles per segment)
* `bento_keccak_po2` (default: `17`): Keccak size (number of r0vm cycles per keccak)
* `bento_rust_log` (default: `"info"`): Rust logging level

#### Database Configuration

**Important**: Bento shares credentials with the PostgreSQL role. Set the `postgresql_*` variables (not `bento_postgresql_*`) and they will be automatically used by both roles.

The bento role uses these variables internally (you typically don't need to set them):

* `bento_postgresql_user`: Maps to `postgresql_user`
* `bento_postgresql_password`: Maps to `postgresql_password`
* `bento_postgresql_host`: Maps to `postgresql_host`
* `bento_postgresql_port`: Maps to `postgresql_port`
* `bento_postgresql_database`: Maps to `postgresql_database`

See the `postgresql` role documentation for the actual variables to set.

#### Valkey Configuration

**Important**: Bento shares configuration with the Valkey role. Set the `valkey_*` variables (not `bento_valkey_*`) and they will be automatically used by both roles.

The bento role uses these variables internally (you typically don't need to set them):

* `bento_valkey_host`: Maps to `valkey_host`
* `bento_valkey_port`: Maps to `valkey_port`

See the `valkey` role documentation for the actual variables to set.

#### S3 Configuration

* `bento_s3_bucket`: S3 bucket name (empty by default)
* `bento_s3_access_key`: S3 access key (empty by default)
* `bento_s3_secret_key`: S3 secret key (empty by default)
* `bento_s3_url`: S3 URL (empty by default)

#### Other Configuration

* `bento_rewards_address`: Rewards address (empty by default)
* `bento_povw_log_id`: POVW log ID (empty by default)

## Dependencies

* `postgresql` role (for PostgreSQL installation and configuration)
* `valkey` role (for Valkey/Redis installation and configuration)

The bento role depends on these roles and will automatically install them. Make sure the `postgresql` and `valkey` roles are available in your roles directory.

## Example Playbook

### Basic Installation with Local Services

```yaml
---
- hosts: provers
  become: true
  roles:
    - role: bento
      vars:
        bento_version: "v1.2.0"
        bento_task: "prove"
        # PostgreSQL configuration - these credentials are shared with bento
        postgresql_user: "bento"
        postgresql_password: "secure_password"
        postgresql_database: "bento"
        # Valkey configuration
        valkey_maxmemory: "12gb"
        # S3 configuration
        bento_s3_bucket: "my-bento-bucket"
        bento_s3_access_key: "AKIAIOSFODNN7EXAMPLE"
        bento_s3_secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
```

**Note**: The `postgresql_user` and `postgresql_password` you set here will be:

1. Used by the `postgresql` role to create the database user
2. Automatically used by the `bento` role in the DATABASE\_URL connection string

### Using External Services

```yaml
---
- hosts: provers
  become: true
  roles:
    - role: bento
      vars:
        bento_version: "v1.2.0"
        bento_task: "prove"
        # Disable local installation, use external services
        postgresql_install: false
        valkey_install: false
        # Configure external service endpoints
        bento_postgresql_user: "bento_user"
        bento_postgresql_password: "secure_password"
        bento_postgresql_host: "db.example.com"
        bento_valkey_host: "valkey.example.com"
```

## Features

* **Idempotent**: Safe to run multiple times
* **Service Management**: Properly manages systemd service with handlers
* **Configuration Management**: Creates environment file and service file from templates
* **Directory Management**: Creates necessary directories with proper permissions
* **Clean Installation**: Downloads, extracts, installs binaries, and cleans up temporary files
* **Database Support**: Depends on `postgresql` role for PostgreSQL installation and configuration
* **Valkey Support**: Depends on `valkey` role for Valkey (Redis fork) installation and configuration
* **Credential Matching**: Automatically uses credentials from dependent roles, ensuring consistency

## Handlers

* `Reload systemd`: Reloads systemd daemon after service file changes
* `Restart Bento service`: Restarts the Bento service
* `Start Bento service`: Starts the Bento service
* `Stop Bento service`: Stops the Bento service

## License

BSD/MIT

## Author Information

Boundless
