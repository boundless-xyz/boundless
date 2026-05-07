# Boundless Ansible Setup

Ansible playbooks for deploying Boundless prover and explorer infrastructure using Docker Compose.

## Overview

This setup deploys:

- **Prover nodes**: Bento stack (PostgreSQL, Redis, MinIO, agents, REST API, optional broker/miner)
- **Explorer nodes**: Same stack plus Caddy for TLS and API-key auth
- **Supporting roles**: NVIDIA drivers and CUDA Toolkit, Docker Engine (with optional NVIDIA Container Toolkit), AWS CLI

## Requirements

- Ansible 2.9 or later
- Target hosts running Ubuntu 22.04 or 24.04
- SSH access to target hosts
- Sudo/root privileges on target hosts
- NVIDIA GPU hardware (for GPU proving)

## Directory Structure

```
ansible/
├── prover.yml             # Main deployment playbook (nvidia, docker, prover)
├── test-prover.yml        # bento-cli prover smoke test
├── kailua.yml             # Kailua installer/service playbook
├── inventory.yml          # Runtime inventory (do not commit real data)
├── INVENTORY.md           # Inventory and secret-management guidance
├── ansible.cfg            # Ansible configuration
├── README.md              # This file
├── ENV_VARS.md            # Variable reference
└── roles/
    ├── awscli/            # AWS CLI v2
    ├── docker/            # Docker Engine + optional NVIDIA Container Toolkit
    ├── kailua/            # Kailua binary + systemd launcher
    ├── nvidia/            # NVIDIA drivers and CUDA Toolkit
    └── prover/            # Prover/explorer Docker Compose deployment
```

## Quick Start

### Local Deployment

```bash
cd ansible

# Deploy prover stack
ansible-playbook -i inventory.yml prover.yml

# Deploy to specific host
ansible-playbook -i inventory.yml prover.yml --limit 127.0.0.1

# Run prover smoke test
ansible-playbook -i inventory.yml test-prover.yml

# Deploy Kailua
ansible-playbook -i inventory.yml kailua.yml
```

### Inventory Setup

The inventory uses nested groups and group vars to reduce duplication:

- **all.vars**: `ansible_user`, `prover_version` (apply to every host)
- **Role groups**: `prover`, `explorer`, `nightly`, `release`, plus chain-ID groups (`8453`, `84532`, `11155111`) for targeting

Hosts live in leaf groups (e.g. `explorer_84532_staging_release`, `prover_8453_production_nightly`). Limit patterns use Ansible group syntax:

```bash
# All staging hosts
ansible-playbook -i inventory.yml prover.yml --limit staging

# Staging hosts that are also in the prover group
ansible-playbook -i inventory.yml prover.yml --limit 'staging:&prover'

# All production nightly provers (same as CI nightly job)
ansible-playbook -i inventory.yml prover.yml --limit 'production:&nightly'
```

Example structure:

```yaml
---
all:
  vars:
    ansible_user: ubuntu
    prover_version: main
  children:
    staging:
      children: [explorer_84532_staging_release, prover_84532_staging_nightly]
    # Leaf groups define hosts and host vars
    explorer_84532_staging_release:
      hosts:
        192.0.2.1:
          caddy_domain: "execution.staging.example.com"
          prover_executor_count: 16
          # ...
```

**Do not commit real infrastructure data.** Keep production inventory in a
secure secret store and inject it in CI/CD (for example, `ANSIBLE_INVENTORY`
as base64). See `INVENTORY.md` for details.

## Playbooks

### prover.yml

Deploys the complete prover stack:

1. **nvidia role** - Installs NVIDIA drivers and CUDA Toolkit
2. **docker role** - Installs Docker with NVIDIA Container Toolkit
3. **prover role** - Deploys prover services via Docker Compose

```bash
ansible-playbook -i inventory.yml prover.yml
```

### test-prover.yml

Installs `bento-cli` from a release bundle and runs a prover smoke test:

```bash
ansible-playbook -i inventory.yml test-prover.yml
```

### kailua.yml

Installs and configures the Kailua service:

```bash
ansible-playbook -i inventory.yml kailua.yml
```

## Configuration Variables

### Global (all.vars)

| Variable         | Description                            |
| ---------------- | -------------------------------------- |
| `ansible_user`   | SSH user (e.g. `ubuntu`)               |
| `prover_version` | Git tag/branch to deploy (e.g. `main`) |

### Prover / Explorer (host or role defaults)

| Variable                       | Default      | Description                                                   |
| ------------------------------ | ------------ | ------------------------------------------------------------- |
| `prover_dir`                   | `/opt/bento` | Deployment directory                                          |
| `prover_postgres_password`     | (required)   | PostgreSQL password                                           |
| `prover_minio_root_pass`       | (required)   | MinIO root password                                           |
| `prover_private_key`           | `""`         | Broker private key (prover nodes)                             |
| `prover_povw_log_id`           | `""`         | POVW log contract address                                     |
| `prover_rpc_urls`              | `""`         | RPC URL(s) for broker                                         |
| `prover_agent_dockerfile`      | (varies)     | e.g. `dockerfiles/agent.dockerfile` or `agent.cpu.dockerfile` |
| `prover_docker_compose_invoke` | (varies)     | Services to run (e.g. `exec_agent rest_api caddy`)            |

Explorer hosts also use Caddy variables (`caddy_domain`, `caddy_acme_email`, `caddy_auth_enabled`, `caddy_api_key`, etc.). See role defaults and `ENV_VARS.md`.

## Tags

Use tags for selective deployment:

```bash
# Deploy only NVIDIA drivers
ansible-playbook -i inventory.yml prover.yml --tags nvidia

# Deploy only Docker
ansible-playbook -i inventory.yml prover.yml --tags docker

# Deploy only prover (skip drivers/docker)
ansible-playbook -i inventory.yml prover.yml --tags prover

# Skip NVIDIA driver installation
ansible-playbook -i inventory.yml prover.yml --skip-tags nvidia
```

### Available Tags

| Tag             | Description                     |
| --------------- | ------------------------------- |
| `nvidia`        | NVIDIA drivers and CUDA Toolkit |
| `docker`        | Docker Engine and Compose       |
| `docker-nvidia` | NVIDIA Container Toolkit        |
| `prover`        | Prover Docker Compose stack     |
| `awscli`        | AWS CLI installation            |

## GitHub Actions Checks

CI checks are defined in `.github/workflows/ansible.yml` (repo root). Current
jobs run:

- `ansible-lint`
- syntax checks for `prover.yml`
- YAML validation for `ansible/**/*.yml`

## Service Management

After deployment, manage services on the target host:

```bash
# SSH to host
ssh ubuntu@<host>

# Navigate to deployment directory
cd /opt/bento

# View service status
docker compose ps

# View logs
docker compose logs -f

# View specific service logs
docker compose logs -f gpu_prove_agent

# Restart all services
sudo systemctl restart bento

# Stop services
sudo systemctl stop bento

# Start services
sudo systemctl start bento
```

### Systemd Service

Bento runs as a systemd service (`bento.service`):

```bash
# Check status
systemctl status bento

# View logs via journalctl
journalctl -u bento -f

# Restart
sudo systemctl restart bento
```

## Troubleshooting

### SSH Connection Issues

Test connectivity first:

```bash
ansible all -i inventory.yml -m ping
```

### Check What Would Change

Use `--check` for a dry run:

```bash
ansible-playbook -i inventory.yml prover.yml --check
```

### NVIDIA Driver Issues

After deployment, verify GPU detection:

```bash
ssh ubuntu@<host> "nvidia-smi"
```

If GPUs aren't detected, a reboot may be required:

```bash
ssh ubuntu@<host> "sudo reboot"
```

### Docker Permission Issues

If docker commands fail with permission errors:

```bash
# Add user to docker group
sudo usermod -aG docker ubuntu

# Log out and back in, or reboot
```

### Service Logs

Check service logs for errors:

```bash
# Systemd service logs
journalctl -u bento -n 100

# Docker Compose logs
cd /opt/bento && docker compose logs --tail=100
```

### Common Errors

**"password authentication failed for user"**

- Check `prover_postgres_password` matches what's in the running PostgreSQL container
- May need to recreate PostgreSQL volume: `docker compose down -v && docker compose up -d`

**"No GPUs detected"**

- Ensure NVIDIA drivers are installed: `nvidia-smi`
- Reboot if drivers were just installed
- Check Docker has GPU access: `docker run --rm --gpus all nvidia/cuda:12.0-base nvidia-smi`

**"collect2: fatal error: cannot find 'ld'"**

- Install required linker packages: `sudo apt-get install -y build-essential binutils lld mold`
- This repository requires `mold` for Rust builds.

## Role Documentation

- `roles/awscli/README.md` - AWS CLI v2
- `roles/docker/README.md` - Docker Engine and NVIDIA Container Toolkit
- `roles/kailua/README.md` - Kailua deployment and service launcher
- `roles/nvidia/README.md` - NVIDIA drivers and CUDA
- `roles/prover/README.md` - Prover/explorer Docker Compose deployment
