# Boundless Ansible Setup

This Ansible playbook replicates the functionality of `scripts/setup.sh` for automated system provisioning.

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
├── prover.yml            # Full prover deployment (bento + broker + dependencies)
├── bento-worker.yml      # Bento worker deployment (no dependencies)
├── broker.yml            # Broker deployment
├── inventory.yml          # Inventory file
├── ansible.cfg            # Ansible configuration
├── README.md              # This file
└── roles/
    ├── bento/             # Bento agent service
    ├── broker/            # Broker service
    ├── postgresql/         # PostgreSQL database
    ├── valkey/             # Valkey (Redis) server
    ├── minio/              # MinIO S3-compatible storage
    ├── rust/               # Rust programming language
    └── rzup/               # RISC Zero rzup toolchain
```

## Installation

Before running playbooks, install required Ansible collections:

```bash
ansible-galaxy collection install community.postgresql
```

## Secrets Management

Secrets can be provided via **environment variables** (recommended), Ansible Vault, or playbook variables.

### Quick Start with Environment Variables

1. **Copy the example environment file:**
   ```bash
   cp .env.example .env
   ```

2. **Edit `.env` with your secrets:**
   ```bash
   export POSTGRESQL_PASSWORD="secure_password"
   export BROKER_PRIVATE_KEY="0x..."
   export BROKER_RPC_URL="https://..."
   ```

3. **Source it and run playbooks:**
   ```bash
   source .env
   ansible-playbook -i inventory.yml broker.yml
   ```

### Other Options

* **Ansible Vault**: See [VAULT.md](VAULT.md) for encrypted vault files
* **AWS Secrets Manager**: Pull secrets from AWS (see [ENV\_VARS.md](ENV_VARS.md))
* **Bitwarden/1Password**: Use CLI tools to export to env vars (see [ENV\_VARS.md](ENV_VARS.md))

See [ENV\_VARS.md](ENV_VARS.md) for complete documentation on environment variable usage.

## Usage

### Full Prover Deployment

Deploy the complete prover stack (Bento + Broker + Dependencies):

```bash
ansible-playbook -i inventory.yml prover.yml
```

### Worker Node Deployment (No Dependencies)

Deploy Bento to worker nodes that connect to remote services:

```bash
# Deploy bento-prove to worker nodes (skips PostgreSQL, Valkey, MinIO)
ansible-playbook -i inventory.yml bento-worker.yml -e bento_task=prove

# Deploy specific Bento task without dependencies
ansible-playbook -i inventory.yml prover.yml --tags bento-prove --skip-tags bento-deps
```

### Using Tags

The Bento role supports tags for selective deployment:

```bash
# Deploy only Bento (skip dependencies)
ansible-playbook -i inventory.yml prover.yml --skip-tags bento-deps

# Deploy only dependencies (skip Bento)
ansible-playbook -i inventory.yml prover.yml --tags bento-deps

# Deploy only Bento installation (skip config/service)
ansible-playbook -i inventory.yml prover.yml --tags bento-install

# Deploy only Bento configuration (skip installation)
ansible-playbook -i inventory.yml prover.yml --tags bento-config,bento-service

# Deploy specific Bento task
ansible-playbook -i inventory.yml prover.yml --tags bento-prove
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
* `postgresql` - PostgreSQL role
* `valkey` - Valkey role
* `minio` - MinIO role
* `broker` - Broker role
* `rust` - Rust programming language role
* `rzup` - RISC Zero rzup toolchain role

### Running on specific hosts

```bash
ansible-playbook -i inventory.yml prover.yml --limit prover-1
```

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
