# Prover Role

This Ansible role deploys the Boundless prover stack using Docker Compose as a systemd-managed service.

## What This Role Does

1. **Clones the Boundless repository** to the deployment directory
2. **Generates the `.env` file** with configuration from Ansible variables
3. **Downloads broker configuration** (`broker.toml`) from the repository
4. **Creates a systemd service** to manage Docker Compose
5. **Starts the prover service**

## Requirements

- Docker Engine (installed via the `docker` role)
- Docker Compose plugin (installed via the `docker` role)
- NVIDIA Container Toolkit (installed via the `docker` role with `docker_nvidia_enabled: true`)
- NVIDIA GPU drivers (installed via the `nvidia` role)

## Role Variables

### Deployment Configuration

| Variable         | Default      | Description              |
| ---------------- | ------------ | ------------------------ |
| `prover_dir`     | `/opt/bento` | Deployment directory     |
| `prover_version` | `v1.2.0`     | Git tag/branch to deploy |
| `prover_state`   | `started`    | Service state            |
| `prover_user`    | `ubuntu`     | User for Docker commands |

### Database Configuration

| Variable                   | Default    | Description         |
| -------------------------- | ---------- | ------------------- |
| `prover_postgres_host`     | `postgres` | PostgreSQL hostname |
| `prover_postgres_port`     | `5432`     | PostgreSQL port     |
| `prover_postgres_db`       | `taskdb`   | Database name       |
| `prover_postgres_user`     | `worker`   | Database user       |
| `prover_postgres_password` | `password` | Database password   |

### Redis Configuration

| Variable            | Default | Description           |
| ------------------- | ------- | --------------------- |
| `prover_redis_host` | `redis` | Redis/Valkey hostname |

### MinIO Configuration

| Variable                 | Default    | Description         |
| ------------------------ | ---------- | ------------------- |
| `prover_minio_host`      | `minio`    | MinIO hostname      |
| `prover_minio_bucket`    | `workflow` | S3 bucket name      |
| `prover_minio_root_user` | `admin`    | MinIO root user     |
| `prover_minio_root_pass` | `password` | MinIO root password |

### Prover Configuration

| Variable                  | Default            | Description              |
| ------------------------- | ------------------ | ------------------------ |
| `prover_rust_log`         | `info`             | Rust log level           |
| `prover_risc0_home`       | `/usr/local/risc0` | RISC0 home directory     |
| `prover_segment_size`     | `20`               | Segment size for proving |
| `prover_risc0_keccak_po2` | `17`               | Keccak power of 2        |
| `prover_redis_ttl`        | `57600`            | Redis TTL in seconds     |
| `prover_snark_timeout`    | `180`              | SNARK timeout in seconds |

### Broker Configuration

| Variable                 | Default      | Description                 |
| ------------------------ | ------------ | --------------------------- |
| `prover_private_key`     | `""`         | Prover wallet private key   |
| `prover_rpc_url`         | `""`         | RPC URL for blockchain      |
| `prover_rpc_urls`        | `""`         | Comma-separated RPC URLs    |
| `prover_povw_log_id`     | `""`         | POVW log contract address   |
| `prover_broker_toml_url` | (GitHub URL) | URL to broker.toml template |

## Usage

### Basic Deployment

```bash
ansible-playbook -i inventory.yml prover.yml
```

### With Custom Variables

Set variables in your inventory file:

```yaml
all:
  hosts:
    my-prover:
      ansible_user: ubuntu
      prover_version: v1.2.0
      prover_postgres_password: "secure_password"
      prover_minio_root_pass: "secure_password"
      prover_private_key: "0x..."
      prover_povw_log_id: "0x..."
      prover_rpc_url: "https://..."
```

Or pass via command line:

```bash
ansible-playbook -i inventory.yml prover.yml \
  -e prover_postgres_password="secure_password"
```

## Service Management

The role creates a systemd service that runs Docker Compose:

```bash
# Check status
systemctl status bento

# View logs
journalctl -u bento -f

# Restart
sudo systemctl restart bento

# Stop
sudo systemctl stop bento
```

### Docker Compose Commands

After deployment, manage services directly:

```bash
cd /opt/bento

# View all services
docker compose ps

# View logs
docker compose logs -f

# View specific service logs
docker compose logs -f gpu_prove_agent

# Restart specific service
docker compose restart gpu_prove_agent
```

## Files Created

| Path                                | Description               |
| ----------------------------------- | ------------------------- |
| `/opt/bento/`                       | Cloned repository         |
| `/opt/bento/.env`                   | Environment configuration |
| `/opt/bento/broker.toml`            | Broker configuration      |
| `/etc/systemd/system/bento.service` | Systemd service unit      |

## Docker Compose Profiles

The service uses Docker Compose profiles:

- **Default**: Core services (postgres, redis, minio, agents, rest\_api)
- **broker**: Adds broker service
- **miner**: Adds miner service

The systemd service runs with `--profile broker --profile miner` by default.

## Troubleshooting

### Service Won't Start

Check the service logs:

```bash
journalctl -u bento -n 100
```

### GPU Not Detected

Verify NVIDIA drivers:

```bash
nvidia-smi
```

Verify Docker can access GPUs:

```bash
docker run --rm --gpus all nvidia/cuda:12.0-base nvidia-smi
```

### Database Connection Errors

Check the `.env` file has correct credentials:

```bash
cat /opt/bento/.env | grep POSTGRES
```

If credentials changed, you may need to recreate the database volume:

```bash
cd /opt/bento
docker compose down -v
docker compose up -d
```

### Redeploy

To force a fresh deployment:

```bash
ansible-playbook -i inventory.yml prover.yml --tags prover
```

## Tags

- `prover` - All prover tasks
- `docker` - Also triggers prover deployment

## Dependencies

This role should be run after:

1. `nvidia` role - Installs GPU drivers
2. `docker` role - Installs Docker with NVIDIA support
