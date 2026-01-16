# Boundless Ansible Setup

Ansible playbooks for deploying Boundless prover infrastructure using Docker Compose.

## Overview

This setup deploys the Boundless prover stack using Docker Compose, including:

- NVIDIA drivers and CUDA Toolkit
- Docker Engine with NVIDIA Container Toolkit
- Prover services (PostgreSQL, Redis, MinIO, agents, REST API)
- Optional broker and miner services
- Vector log shipping to AWS CloudWatch (optional)

## Requirements

- Ansible 2.9 or later
- Target hosts running Ubuntu 22.04 or 24.04
- SSH access to target hosts
- Sudo/root privileges on target hosts
- NVIDIA GPU hardware (for GPU proving)

## Directory Structure

```
ansible/
├── prover.yml             # Main deployment playbook
├── monitoring.yml         # AWS CLI + Vector log shipping
├── inventory.yml          # Host inventory (base64 encoded for CI)
├── ansible.cfg            # Ansible configuration
├── README.md              # This file
├── ENV_VARS.md            # Environment variable documentation
└── roles/
    ├── awscli/            # AWS CLI v2 installation
    ├── docker/            # Docker Engine + NVIDIA Container Toolkit
    ├── nvidia/            # NVIDIA drivers and CUDA Toolkit
    ├── prover/            # Prover Docker Compose deployment
    └── vector/            # Vector log shipping to CloudWatch
```

## Quick Start

### Local Deployment

```bash
cd ansible

# Deploy prover stack
ansible-playbook -i inventory.yml prover.yml

# Deploy to specific host
ansible-playbook -i inventory.yml prover.yml --limit 127.0.0.1

# Deploy monitoring (Vector + AWS CLI)
ansible-playbook -i inventory.yml monitoring.yml
```

### Inventory Setup

The inventory file defines hosts and their configuration:

```yaml
---
all:
  children:
    nightly:
      hosts:
        127.0.0.1:
          ansible_user: ubuntu
          prover_version: main
          prover_private_key: "0x..."
          prover_povw_log_id: "0x..."
          prover_rpc_url: "https://..."
          prover_postgres_password: "secure_password"
          prover_minio_root_pass: "secure_password"
    release:
      hosts:
        10.0.0.2:
          ansible_user: ubuntu
          prover_version: v1.2.0
          # ... other variables
```

## Playbooks

### prover.yml

Deploys the complete prover stack:

1. **nvidia role** - Installs NVIDIA drivers and CUDA Toolkit
2. **docker role** - Installs Docker with NVIDIA Container Toolkit
3. **prover role** - Deploys prover services via Docker Compose

```bash
ansible-playbook -i inventory.yml prover.yml
```

### monitoring.yml

Deploys log shipping to AWS CloudWatch:

1. **awscli role** - Installs AWS CLI v2
2. **vector role** - Installs and configures Vector

```bash
ansible-playbook -i inventory.yml monitoring.yml
```

## Configuration Variables

### Prover Configuration

Key variables (set in inventory or via `-e`):

| Variable                   | Default      | Description               |
| -------------------------- | ------------ | ------------------------- |
| `prover_version`           | `v1.2.0`     | Git tag/branch to deploy  |
| `prover_dir`               | `/opt/bento` | Deployment directory      |
| `prover_postgres_password` | `password`   | PostgreSQL password       |
| `prover_minio_root_pass`   | `password`   | MinIO root password       |
| `prover_private_key`       | `""`         | Broker private key        |
| `prover_povw_log_id`       | `""`         | POVW log contract address |
| `prover_rpc_url`           | `""`         | RPC URL for broker        |

### Vector Configuration

| Variable                       | Default                     | Description                            |
| ------------------------------ | --------------------------- | -------------------------------------- |
| `vector_service_enabled`       | `false`                     | Enable Vector at boot                  |
| `vector_service_state`         | `stopped`                   | Service state                          |
| `vector_cloudwatch_log_group`  | `/boundless/bento/hostname` | CloudWatch log group                   |
| `vector_cloudwatch_region`     | `us-west-2`                 | AWS region                             |
| `vector_aws_access_key_id`     | `null`                      | AWS access key (if not using IAM role) |
| `vector_aws_secret_access_key` | `null`                      | AWS secret key (if not using IAM role) |

See `ENV_VARS.md` for complete variable documentation.

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
| `vector`        | Vector log shipping             |

## GitHub Actions Deployment

The repository includes automated deployment via GitHub Actions (`.github/workflows/prover-deploy.yml`).

### Host Groups

- **staging**: Deployed manually for testing (Base Sepolia)
- **nightly**: Deployed daily at 1am UTC from `main` branch (Base Mainnet)
- **release**: Deployed manually using official release tags (Base Mainnet)

### Required Secrets

| Secret                    | Description                  |
| ------------------------- | ---------------------------- |
| `ANSIBLE_SSH_PRIVATE_KEY` | SSH private key (ed25519)    |
| `ANSIBLE_INVENTORY`       | Base64-encoded inventory.yml |

### Creating the Inventory Secret

```bash
cd ansible
base64 -i inventory.yml | tr -d '\n'
# Copy output to GitHub secret: ANSIBLE_INVENTORY
```

### Manual Deployment

1. Go to Actions → Prover Deployment
2. Click "Run workflow"
3. Select:
   - **Target**: `staging`, `nightly`, `release`, or `all`
   - **Playbook**: `prover.yml` or `monitoring.yml`

### Scheduled Deployment

The nightly host is automatically deployed at 1am UTC from the `main` branch.

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

## Role Documentation

See individual role READMEs for detailed documentation:

- `roles/awscli/README.md` - AWS CLI installation
- `roles/docker/README.md` - Docker installation
- `roles/nvidia/README.md` - NVIDIA drivers
- `roles/prover/README.md` - Prover Docker Compose deployment
- `roles/vector/README.md` - Vector log shipping
