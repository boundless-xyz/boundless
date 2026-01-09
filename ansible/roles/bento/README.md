# Bento Role

This Ansible role deploys Bento services using Docker Compose, providing a standalone deployment option that's ideal for:

- Development environments
- Single-node deployments
- Quick testing and evaluation
- Local development setups

## Differences from Cluster Deployment

Unlike the `bento` role used in `cluster.yml`, which deploys services as systemd units, this role:

- Uses Docker Compose to manage all services
- Runs services in containers instead of native systemd services
- Simplifies dependency management (all services containerized)
- Easier to tear down and recreate

## Requirements

- Docker Engine (installed via the `docker` role)
- Docker Compose plugin (installed via the `docker` role)
- Sufficient system resources for containerized services

## Role Variables

### Service Configuration

```yaml
# Enable/disable services
bento_docker_services:
  postgres: true
  redis: true
  minio: true
  rest_api: true
  exec_agent: true
  aux_agent: true
  gpu_prove_agent0: false  # Set to true if GPU available
  grafana: false
  broker: false
  miner: false

# Service state: started, stopped, restarted, absent
bento_docker_state: started
```

### Environment Variables

See `defaults/main.yml` for full list of environment variables. Key variables include:

```yaml
bento_docker_env:
  POSTGRES_DB: "taskdb"
  POSTGRES_USER: "worker"
  POSTGRES_PASSWORD: "password"
  RUST_LOG: "info"
  SEGMENT_SIZE: "20"
  RISC0_KECCAK_PO2: "17"
  REDIS_TTL: "57600"
  BENTO_BINARY_URL: "https://github.com/boundless-xyz/boundless/releases/download/bento-v1.2.0/bento-bundle-linux-amd64.tar.gz"
```

## Usage

### Basic Deployment

```bash
ansible-playbook -i inventory.yml bento-standalone.yml
```

### With Custom Variables

```bash
ansible-playbook -i inventory.yml bento-standalone.yml \
  -e bento_docker_services.gpu_prove_agent0=true
```

### Stop Services

```bash
ansible-playbook -i inventory.yml bento-standalone.yml \
  -e bento_docker_state=stopped
```

### Remove Services

```bash
ansible-playbook -i inventory.yml bento-standalone.yml \
  -e bento_docker_state=absent
```

## Manual Docker Compose Commands

After deployment, you can also manage services manually:

```bash
# SSH to the target host
ssh user@target-host

# Navigate to deployment directory
cd /opt/bento

# View service status
docker compose ps

# View logs
docker compose logs -f

# Restart services
docker compose restart

# Stop services
docker compose stop

# Start services
docker compose start
```

## Notes

- **Docker Installation**: Docker and Docker Compose must be installed separately using the `docker` role before running this role
- The compose.yml file is templated and generated from the role
- Environment variables are stored in `.env` file in the deployment directory
- Services are conditionally included based on `bento_docker_services` configuration
- Ensure the user specified in `bento_docker_user` is in the docker group (can be configured via the `docker` role's `docker_users` variable)
