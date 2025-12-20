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
* `bento_count` (default: `1`): Number of service instances to deploy (when > 1, launcher script starts multiple instances in parallel)
* `bento_install_dependencies` (default: `true`): Whether to install PostgreSQL, Valkey, and MinIO on this host

#### Service Configuration

* `bento_service_enabled` (default: `true`): Enable service to start on boot
* `bento_service_state` (default: `started`): Service state (started, stopped, restarted)
* `bento_agent_timeout_start_sec` (default: `1800`): Service start timeout in seconds
* `bento_agent_timeout_stop_sec` (default: `300`): Service stop timeout in seconds

#### Bento Configuration

* `bento_segment_po2` (default: `20`): Segment size parameter
* `bento_keccak_po2` (default: `17`): Keccak size parameter
* `bento_rust_log` (default: `"info"`): Rust logging level
* `bento_prometheus_metrics_addr` (default: `"127.0.0.1:9090"`): Prometheus metrics endpoint

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

**Important**: S3 configuration variables reference MinIO settings by default. Override in `host_vars` for remote connections.

* `bento_s3_bucket` (default: `"bento"`): S3 bucket name
* `bento_s3_access_key` (default: `{{ minio_root_user }}`): S3 access key (matches MinIO root user)
* `bento_s3_secret_key` (default: `{{ minio_root_password }}`): S3 secret key (matches MinIO root password)
* `bento_s3_url` (default: `http://{{ minio_host }}:{{ minio_port }}`): S3 URL
* `bento_s3_region` (default: `"auto"`): S3 region

#### Other Configuration

* `bento_rewards_address` (default: `""`): Rewards address
* `bento_povw_log_id` (default: `""`): POVW log ID
* `bento_risc0_home_service`: RISC0\_HOME directory for the service (defaults to `{{ bento_home }}/.risc0`, automatically set by rzup role)
* `bento_gpu_count` (default: `1`): Number of GPU prove workers
* `bento_exec_count` (default: `4`): Number of exec workers
* `bento_aux_count` (default: `2`): Number of aux workers

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
        # S3 configuration (uses MinIO credentials by default)
        minio_root_user: "minioadmin"
        minio_root_password: "secure_password"
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
        bento_install_dependencies: false  # Skip local service installation
        # Configure external service endpoints
        postgresql_install: false
        postgresql_host: "db.example.com"
        postgresql_user: "bento_user"
        postgresql_password: "secure_password"
        valkey_install: false
        valkey_host: "valkey.example.com"
        minio_install: false
        minio_host: "s3.example.com"
        minio_root_user: "s3_access_key"
        minio_root_password: "s3_secret_key"
```

**Better approach**: Set these in `host_vars/HOSTNAME/main.yml` for persistent configuration:

```yaml
# host_vars/prover-01/main.yml
postgresql_install: false
postgresql_host: "db.example.com"
valkey_install: false
valkey_host: "valkey.example.com"
minio_install: false
minio_host: "s3.example.com"
bento_install_dependencies: false
```

## Features

* **Idempotent**: Safe to run multiple times
* **Version Tracking**: Tracks installed Bento version in `/etc/boundless/.bento_version` and only reinstalls when version changes or binaries are missing
* **Launcher Script Architecture**: Uses launcher scripts to start multiple instances per service type, simplifying systemd configuration
* **Service Management**: Properly manages systemd service with handlers, supports multiple instances via launcher scripts
* **Smart Restart Logic**: Services restart only when binaries or configuration files change, not on cleanup tasks or version file updates
* **Configuration Management**: Creates environment file, launcher scripts, and service file from templates
* **Directory Management**: Creates necessary directories with proper permissions (shared `/etc/boundless/` directory)
* **Clean Installation**: Downloads, extracts, installs binaries, and cleans up temporary files
* **Database Support**: Integrates with `postgresql` role for PostgreSQL installation and configuration
* **Valkey Support**: Integrates with `valkey` role for Valkey (Redis fork) installation and configuration
* **S3 Support**: Integrates with `minio` role for MinIO S3-compatible storage (credentials automatically synchronized)
* **Rust Support**: Automatically installs Rust for the bento user via `rust` role
* **RISC Zero Support**: Automatically installs rzup and risc0-groth16 for Groth16 proof generation via `rzup` role
* **Credential Matching**: Automatically uses credentials from dependent roles, ensuring consistency
* **Ansible Variable System**: All configuration variables follow Ansible's standard variable precedence (host\_vars → group\_vars → defaults)
* **Prometheus Metrics**: Configurable Prometheus metrics endpoint (defaults to `127.0.0.1:9090`)
* **Dedicated User**: Creates a dedicated `bento` system user with proper directory structure and permissions
* **Security**: Service runs as non-root user with appropriate file permissions and systemd security settings
* **Worker Node Support**: Can deploy to worker nodes without local dependencies by setting `bento_install_dependencies: false`

## Handlers

* `Reload systemd`: Reloads systemd daemon after service file changes
* `Restart Bento service`: Restarts the specific Bento service (e.g., `bento-prove`, `bento-api`)
* `Start Bento service`: Starts the Bento service
* `Stop Bento service`: Stops the Bento service
* `Restart all Bento services`: Restarts all Bento services (legacy handler, not used in current implementation)

**Note**: Handlers are triggered only when:

* Binary files are installed or updated
* Configuration files (environment file, launcher script, service file) are changed
* Cleanup tasks and version file updates do NOT trigger service restarts

## Service Architecture

### Launcher Script Approach

Each Bento service type uses a launcher script that:

1. Starts multiple instances in parallel (when `bento_count > 1`)
2. Handles GPU assignment for prove workers (`CUDA_VISIBLE_DEVICES`)
3. Waits for all processes to complete

**Launcher Scripts**:

* `/etc/boundless/bento-prove-launcher.sh` - Starts GPU prove workers
* `/etc/boundless/bento-api-launcher.sh` - Starts REST API
* `/etc/boundless/bento-exec-launcher.sh` - Starts exec workers
* `/etc/boundless/bento-aux-launcher.sh` - Starts aux workers
* `/etc/boundless/bento-snark-launcher.sh` - Starts SNARK compression workers
* `/etc/boundless/bento-join-launcher.sh` - Starts join task workers
* `/etc/boundless/bento-union-launcher.sh` - Starts union task workers
* `/etc/boundless/bento-coproc-launcher.sh` - Starts coprocessor workers

**Systemd Units**:

* `/etc/systemd/system/bento-prove.service`
* `/etc/systemd/system/bento-api.service`
* `/etc/systemd/system/bento-exec.service`
* `/etc/systemd/system/bento-aux.service`
* `/etc/systemd/system/bento-snark.service`
* `/etc/systemd/system/bento-join.service`
* `/etc/systemd/system/bento-union.service`
* `/etc/systemd/system/bento-coproc.service`

### Directory Permissions

**Critical**: The `/etc/boundless/` directory must have `0755` permissions and be owned by `root:root` to allow both `bento` and `broker` users to access it. The Bento role automatically sets these permissions to prevent conflicts with the Broker role.

**File Permissions**:

* Config directory (`/etc/boundless/`): `0755`, owned by `root:root` (shared with broker)
* Launcher scripts: `0755`, owned by `bento:bento`
* Environment file (`bento.env`): `0640`, owned by `root:bento`
* Version file (`.bento_version`): `0644`, owned by `root:root` (metadata only, no restart on change)

## Troubleshooting

### Permission Denied Errors

If services fail with "Permission denied" (exit code 126/203):

1. **Check directory permissions**:
   ```bash
   ls -ld /etc/boundless/
   # Should show: drwxr-xr-x root root
   # Fix: sudo chmod 0755 /etc/boundless && sudo chown root:root /etc/boundless
   ```

2. **Check script permissions**:
   ```bash
   ls -l /etc/boundless/bento-*-launcher.sh
   # Should show: -rwxr-xr-x bento bento
   # Fix: sudo chmod +x /etc/boundless/bento-*-launcher.sh
   ```

3. **Test script manually**:
   ```bash
   sudo -u bento /etc/boundless/bento-aux-launcher.sh
   ```

4. **Reset systemd state**:
   ```bash
   sudo systemctl reset-failed bento-{task}
   sudo systemctl daemon-reload
   ```

### Service Restart Loops

Check logs for errors:

```bash
journalctl -u bento-{task} -n 50
```

Common causes:

* Script exits immediately (check launcher script)
* Binary not found (check `bento_install_dir`)
* Environment variables missing (check `bento.env`)
* Port conflicts (for API service)

### Multiple Instances Not Starting

Verify launcher script uses `for` loop with `&` to background processes and `wait` at end.

### Check Service Status

```bash
systemctl status bento-prove bento-api bento-exec bento-aux
journalctl -u bento-prove -f
```

## License

BSD/MIT

## Author Information

Boundless
