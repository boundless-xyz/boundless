# Boundless Ansible Setup

This Ansible playbook replicates the functionality of the repo root `scripts/setup.sh` for automated system provisioning.

## Overview

The playbook sets up a Boundless prover system by:

* Updating system packages
* Installing essential packages (nvtop, build tools, etc.)
* Installing GCC 12 for Ubuntu 22.04
* Installing Docker with NVIDIA Container Toolkit support
* Installing Rust programming language (via `rust` role)
* Installing RISC Zero rzup toolchain and risc0-groth16 component (via `rzup` role)
* Installing Just command runner
* Installing CUDA Toolkit 13.0
* Installing Protobuf compiler
* Performing system cleanup

## Requirements

* Ansible 2.9 or later
* Target hosts running Ubuntu 22.04 or 24.04
* SSH access to target hosts (or run locally)
* Sudo/root privileges on target hosts
* Ansible collections (install with `ansible-galaxy collection install community.postgresql`)

## Directory Structure

```
ansible/
├── cluster.yml            # Full cluster deployment (bento + broker + dependencies)
├── bento-worker.yml       # Bento worker deployment (no dependencies)
├── broker.yml             # Broker deployment
├── inventory.yml          # Inventory file
├── ansible.cfg            # Ansible configuration
├── README.md              # This file
├── ENV_VARS.md            # Environment variable documentation
└── roles/
    ├── awscli/             # AWS CLI v2 installation
    ├── bento/              # Bento agent service (uses launcher scripts)
    ├── broker/             # Broker service
    ├── grafana/            # Grafana monitoring
    ├── miner/              # Miner service
    ├── minio/              # MinIO S3-compatible storage
    ├── nvidia/             # NVIDIA drivers and CUDA Toolkit
    ├── postgresql/         # PostgreSQL database
    ├── rust/               # Rust programming language
    ├── rzup/               # RISC Zero rzup toolchain
    ├── valkey/             # Valkey (Redis) server
    └── vector/             # Vector log shipping to CloudWatch
```

## Installation

Before running playbooks, install required Ansible collections:

```bash
ansible-galaxy collection install community.postgresql
```

## Quick Deployment

From this directory, run:

```bash
# Full cluster deployment (bento + broker + dependencies)
ansible-playbook -i inventory.yml cluster.yml

# Bento worker deployment (no dependencies)
ansible-playbook -i inventory.yml bento-worker.yml
```

## Configuration Management

Configuration is managed using Ansible's built-in variable system. Variables can be set at multiple levels with the following precedence (highest to lowest):

1. **Command-line variables** (`-e var=value`)
2. **Host variables** (`host_vars/HOSTNAME/main.yml` or `host_vars/HOSTNAME/vault.yml`)
3. **Group variables** (`group_vars/all/*.yml`)
4. **Role defaults** (`roles/*/defaults/main.yml`)

### Quick Start with Host Variables

1. **Create host-specific configuration:**
   ```bash
   # Edit host_vars/example/main.yml for non-sensitive config
   nano ansible/host_vars/example/main.yml
   ```

2. **Set sensitive values in vault:**
   ```bash
   # Create encrypted vault file
   ansible-vault create ansible/host_vars/example/vault.yml
   ```

3. **Or use command-line variables:**
   ```bash
   ansible-playbook -i inventory.yml broker.yml \
     -e postgresql_password="secure_password" \
     -e broker_private_key="0x..."
   ```

### Remote Worker Node Configuration

For worker nodes connecting to remote services, set these in `host_vars/HOSTNAME/main.yml`:

```yaml
# Disable local service installation
postgresql_install: false
valkey_install: false
minio_install: false
bento_install_dependencies: false

# Point to remote services
postgresql_host: "10.0.1.10"  # Manager node
valkey_host: "10.0.1.10"
minio_host: "10.0.1.10"
```

Sensitive values (passwords, keys) should be set in `host_vars/HOSTNAME/vault.yml` or passed via `-e` flags.

See `host_vars/example/main.yml` for a complete example.

## Usage

### Full Cluster Deployment

Deploy the complete prover stack (Bento + Broker + Dependencies):

```bash
ansible-playbook -i inventory.yml cluster.yml
```

### Worker Node Deployment (No Dependencies)

Deploy Bento to worker nodes that connect to remote services:

```bash
# Deploy Bento to worker nodes (skips PostgreSQL, Valkey, MinIO)
ansible-playbook -i inventory.yml bento-worker.yml

# Deploy specific Bento task without dependencies
ansible-playbook -i inventory.yml cluster.yml --tags bento-prove --skip-tags bento-deps
```

### Using Tags

The Bento role supports tags for selective deployment:

```bash
# Deploy only Bento (skip dependencies)
ansible-playbook -i inventory.yml cluster.yml --skip-tags bento-deps

# Deploy only dependencies (skip Bento)
ansible-playbook -i inventory.yml cluster.yml --tags bento-deps

# Deploy only Bento installation (skip config/service)
ansible-playbook -i inventory.yml cluster.yml --tags bento-install

# Deploy only Bento configuration (skip installation)
ansible-playbook -i inventory.yml cluster.yml --tags bento-config,bento-service

# Deploy specific Bento task
ansible-playbook -i inventory.yml cluster.yml --tags bento-prove

# Deploy Grafana monitoring
ansible-playbook -i inventory.yml cluster.yml --tags grafana
```

### Available Tags

* `bento` - All Bento tasks
* `bento-deps` - Dependencies (PostgreSQL, Valkey, MinIO)
* `bento-user` - User/group creation
* `bento-install` - Binary installation
* `bento-config` - Configuration files and directories
* `bento-service` - Systemd service management
* `bento-prove` - Bento prove task deployment
* `bento-api` - Bento API deployment
* `bento-exec` - Bento exec task deployment
* `bento-aux` - Bento aux task deployment
* `bento-snark` - Bento snark task deployment
* `bento-join` - Bento join task deployment
* `bento-union` - Bento union task deployment
* `bento-coproc` - Bento coproc task deployment
* `awscli` - AWS CLI installation
* `nvidia` - NVIDIA drivers and CUDA Toolkit
* `postgresql` - PostgreSQL role
* `valkey` - Valkey role
* `minio` - MinIO role
* `broker` - Broker role
* `miner` - Miner service role
* `vector` - Vector log shipping role
* `rust` - Rust programming language role
* `rzup` - RISC Zero rzup toolchain role

Note: Bento worker enablement is controlled by `bento_*_count` and `bento_*_workers` variables, not tags.

### Running on specific hosts

```bash
ansible-playbook -i inventory.yml cluster.yml --limit prover-1
```

## Service Architecture

### Bento Services

Bento services use a **unified launcher script** managed by a single systemd unit:

* Systemd unit: `/etc/systemd/system/bento.service`
* Launcher script: `/etc/boundless/bento-launcher.sh`

The launcher starts the API (if enabled) and all configured worker types in parallel based on `bento_*` counts and flags.

**Worker Types**:

* `bento-prove`: GPU proving workers
* `bento-api`: REST API service
* `bento-exec`: Execution workers
* `bento-aux`: Auxiliary workers
* `bento-snark`: SNARK compression workers (when `SNARK_STREAM=1`)
* `bento-join`: Join task workers (when `JOIN_STREAM=1`)
* `bento-union`: Union task workers (when `UNION_STREAM=1`)
* `bento-coproc`: Coprocessor workers (when `COPROC_STREAM=1`)

### Shared Configuration Directory

Both Bento and Broker services use `/etc/boundless/` as a shared configuration directory. This directory must have `0755` permissions and be owned by `root:root` to allow both services to access it.

**Important**: The Bento role manages this directory and sets it to `root:root` with `0755` permissions. The Broker role also ensures this directory exists with the same permissions to prevent conflicts.

## Differences from setup.sh

1. **Git submodules**: The Ansible playbook attempts to initialize git submodules, but this is best done manually or in a separate task since it requires the repository to be present.

2. **Interactive reboot**: The bash script prompts for reboot, but Ansible doesn't handle interactive prompts. You can reboot manually or add a separate task.

3. **Logging**: The bash script logs to `/var/log/setup.log`. Ansible has its own logging mechanisms.

## Notes

* The playbook is idempotent - you can run it multiple times safely
* Some tasks require a reboot to take full effect (especially GPU drivers and Docker group membership)
* For Ubuntu 22.04 vs 24.04, different installation methods are used for Rust (rustup installer script vs apt package)
* The `rust` and `rzup` roles are automatically included when deploying Bento to ensure Groth16 proof generation works correctly
* Docker group membership changes require logging out and back in, or a reboot
* **Version Tracking**: The Bento role tracks installed versions in `/etc/boundless/.bento_version` and only reinstalls when the version changes or binaries are missing
* **Service Restart Logic**: Services restart only when binaries or configuration files change, not on cleanup tasks or version file updates

## Troubleshooting

### Permission issues

Ensure your user has sudo privileges. If sudo requires a password, use `-K` or `--ask-become-pass`:

```bash
# Prompt for sudo password
ansible-playbook playbook.yml -K

# Or if you also need SSH password
ansible-playbook playbook.yml -k -K
```

**Note**: `-k` is for SSH password, `-K` is for sudo/become password.

### Service Permission Denied Errors

If Bento services fail with "Permission denied" errors:

1. **Check directory permissions**:
   ```bash
   ls -ld /etc/boundless/
   # Should be: drwxr-xr-x root root
   # Fix if needed: sudo chmod 0755 /etc/boundless && sudo chown root:root /etc/boundless
   ```

2. **Check launcher script permissions**:
   ```bash
   ls -l /etc/boundless/bento-launcher.sh
   # Should be: -rwxr-xr-x bento bento
   # Fix if needed: sudo chmod +x /etc/boundless/bento-launcher.sh
   ```

3. **Reset systemd state**:
   ```bash
   sudo systemctl reset-failed bento
   sudo systemctl daemon-reload
   sudo systemctl restart bento
   ```

See the role-specific README files for more detailed troubleshooting.

### Connection issues

Test connectivity first:

```bash
ansible all -i inventory.yml -m ping
```

### Check what would change

Use `--check` for a dry run:

```bash
ansible-playbook playbook.yml --check
```

### Service Debugging

Check service status and logs:

```bash
# On the target host
systemctl status bento
journalctl -u bento -f
journalctl -u bento -n 100
```

For detailed troubleshooting, see:

* `roles/awscli/README.md` - AWS CLI installation troubleshooting
* `roles/bento/README.md` - Bento service troubleshooting
* `roles/broker/README.md` - Broker service troubleshooting
* `roles/vector/README.md` - Vector log shipping troubleshooting
