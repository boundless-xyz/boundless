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

* `bento_version` (default: `"v1.1.2"`): Version of Bento to install
* `bento_task` (default: `"prove"`): Task type for the agent (prove, execute, aux, etc.)
* `bento_user` (default: `"bento"`): User to run the service as (dedicated service user)
* `bento_group` (default: `"bento"`): Group for the bento user
* `bento_home` (default: `"/var/lib/bento"`): Home directory for the bento user
* `bento_shell` (default: `"/usr/sbin/nologin"`): Shell for the bento user (nologin for security)
* `bento_install_dir` (default: `"/usr/local/bin"`): Directory to install binaries
* `bento_config_dir` (default: `"/etc/boundless"`): Directory for configuration files (root-owned, readable by bento group)
* `bento_work_dir` (default: `"/var/lib/bento"`): Working directory for the service
* `bento_count` (default: `1`): Number of service instances to deploy (when > 1, uses systemd template units)
* `bento_install_dependencies` (default: `true`): Whether to install PostgreSQL, Valkey, and MinIO on this host

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

**Important**: S3 configuration variables are defined in `group_vars/all/main.yml` to ensure consistency. They can be overridden via environment variables.

* `bento_s3_bucket`: S3 bucket name (defaults to `"bento"` or `BENTO_S3_BUCKET` env var)
* `bento_s3_access_key`: S3 access key (defaults to MinIO root user or `BENTO_S3_ACCESS_KEY` env var)
* `bento_s3_secret_key`: S3 secret key (defaults to MinIO root password or `BENTO_S3_SECRET_KEY` env var)
* `bento_s3_url`: S3 URL (defaults to `http://{minio_host}:{minio_port}` or `BENTO_S3_URL` env var)
* `bento_s3_region`: S3 region (defaults to `"auto"` or `BENTO_S3_REGION` env var)

#### Other Configuration

* `bento_rewards_address`: Rewards address (empty by default, can be set via `BENTO_REWARDS_ADDRESS` env var)
* `bento_povw_log_id`: POVW log ID (empty by default, can be set via `POVW_LOG_ID` env var)
* `bento_risc0_home_service`: RISC0\_HOME directory for the service (defaults to `{{ bento_home }}/.risc0`, automatically set by rzup role)

## Dependencies

The bento role includes the following roles when `bento_install_dependencies` is `true`:

* `postgresql` role (for PostgreSQL installation and configuration)
* `valkey` role (for Valkey/Redis installation and configuration)
* `minio` role (for MinIO S3-compatible storage installation and configuration)

Additionally, the bento role always includes:

* `rust` role (for Rust programming language installation for the bento user)
* `rzup` role (for RISC Zero rzup toolchain and risc0-groth16 component installation)

These roles are automatically included when deploying Bento. Make sure all required roles are available in your roles directory.

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
        # S3 configuration (or set via environment variables)
        # bento_s3_bucket, bento_s3_access_key, bento_s3_secret_key, bento_s3_url
        # are defined in group_vars/all/main.yml and can be overridden via env vars
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
* **Service Management**: Properly manages systemd service with handlers, supports multiple instances via template units
* **Configuration Management**: Creates environment file and service file from templates
* **Directory Management**: Creates necessary directories with proper permissions
* **Clean Installation**: Downloads, extracts, installs binaries, and cleans up temporary files
* **Database Support**: Integrates with `postgresql` role for PostgreSQL installation and configuration
* **Valkey Support**: Integrates with `valkey` role for Valkey (Redis fork) installation and configuration
* **S3 Support**: Integrates with `minio` role for MinIO S3-compatible storage
* **Rust Support**: Automatically installs Rust for the bento user via `rust` role
* **RISC Zero Support**: Automatically installs rzup and risc0-groth16 for Groth16 proof generation via `rzup` role
* **Credential Matching**: Automatically uses credentials from dependent roles, ensuring consistency
* **Dedicated User**: Creates a dedicated `bento` system user with proper directory structure and permissions
* **Security**: Service runs as non-root user with appropriate file permissions and systemd security settings
* **Worker Node Support**: Can deploy to worker nodes without local dependencies by setting `bento_install_dependencies: false`

## Handlers

* `Reload systemd`: Reloads systemd daemon after service file changes
* `Restart Bento service`: Restarts the Bento service
* `Start Bento service`: Starts the Bento service
* `Stop Bento service`: Stops the Bento service

## License

BSD/MIT

## Author Information

Boundless
