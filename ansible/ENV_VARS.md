# Configuration Variables

This document describes all configuration variables for the Boundless Ansible deployment. **Do not commit real credentials, IPs, or secrets**—use placeholders in the repo and supply real values via inventory (e.g. CI secret or Vault).

## Variable Precedence

Ansible variable precedence (highest to lowest):

1. **Command-line variables** (`-e var=value`)
2. **Inventory host variables** (per-host in `inventory.yml`)
3. **Inventory group variables** (e.g. `staging.vars`, `production.vars`, `all.vars` in `inventory.yml`)
4. **Role defaults** (`roles/*/defaults/main.yml`)

The inventory uses `all.vars` for `ansible_user`, `prover_version`, and `ufw_docker_nat_enabled`. Staging and production set `vector_aws_access_key_id` and `vector_aws_secret_access_key` so hosts in those groups inherit AWS credentials for Vector without per-host duplication.

## Quick Start

### Using Inventory Variables

Variables can be set in `all.vars`, group `vars` (e.g. `staging`, `production`), or per host:

```yaml
all:
  vars:
    ansible_user: ubuntu
    prover_version: main
    ufw_docker_nat_enabled: true
  children:
    staging:
      vars:
        vector_aws_access_key_id: "AKIA..."
        vector_aws_secret_access_key: "..."
      children:
        prover_84532_staging_nightly:
    prover_84532_staging_nightly:
      hosts:
        127.0.0.1:
          prover_postgres_password: "secure_password"
          prover_minio_root_pass: "secure_password"
          prover_private_key: "0x..."
          prover_povw_log_id: "0x..."
          prover_rpc_urls: "https://..."
```

### Using Command-Line Variables

```bash
ansible-playbook -i inventory.yml prover.yml \
  -e prover_postgres_password="secure_password" \
  -e prover_private_key="0x..."
```

## Prover Role Variables

### Deployment Configuration

| Variable         | Default      | Description                                                             |
| ---------------- | ------------ | ----------------------------------------------------------------------- |
| `prover_dir`     | `/opt/bento` | Directory where prover is deployed                                      |
| `prover_version` | `v1.2.1`     | Git tag or branch to deploy (inventory often sets `main` in `all.vars`) |
| `prover_state`   | `started`    | Service state (started, stopped)                                        |
| `prover_user`    | `ubuntu`     | User for Docker commands                                                |

### Dockerfiles and Compose (prover vs explorer)

| Variable                        | Default / typical                       | Description                                                                               |
| ------------------------------- | --------------------------------------- | ----------------------------------------------------------------------------------------- |
| `prover_boundless_build`        | `""`                                    | Set to `"all"` to build from source (clears image vars, adds `--build`)                   |
| `prover_image_version`          | `""`                                    | Image version for pre-built images (e.g. `1.2`); empty uses `latest`                     |
| `prover_agent_dockerfile`       | `""`                                    | Agent Dockerfile override (e.g. `dockerfiles/agent.cpu.dockerfile` for CPU-only)          |
| `prover_rest_api_dockerfile`    | `""`                                    | REST API Dockerfile override                                                              |
| `prover_broker_dockerfile`      | `""`                                    | Broker Dockerfile override                                                                |
| `prover_docker_compose_invoke`  | `""`                                    | Services to run (e.g. `exec_agent rest_api caddy` for explorer)                           |
| `prover_docker_compose_profile` | `--profile broker --profile miner`      | Compose profiles (explorer: `--profile caddy`)                                            |
| `prover_docker_runtime`         | `nvidia`                                | Docker runtime (`runc` for CPU-only explorer)                                             |

Explorer hosts also use Caddy variables: `caddy_domain`, `caddy_acme_email`, `caddy_auth_enabled`, `caddy_api_key`, `prover_caddy_file`. See `inventory.yml` and the prover role templates for current names and usage.

### Database Configuration (PostgreSQL)

| Variable                   | Default    | Description                                  |
| -------------------------- | ---------- | -------------------------------------------- |
| `prover_postgres_host`     | `postgres` | PostgreSQL hostname (Docker service name)    |
| `prover_postgres_port`     | `5432`     | PostgreSQL port                              |
| `prover_postgres_db`       | `taskdb`   | Database name                                |
| `prover_postgres_user`     | `worker`   | Database user                                |
| `prover_postgres_password` | `password` | Database password (**change in production**) |

### Redis/Valkey Configuration

| Variable            | Default | Description                          |
| ------------------- | ------- | ------------------------------------ |
| `prover_redis_host` | `redis` | Redis hostname (Docker service name) |

### MinIO/S3 Configuration

| Variable                 | Default    | Description                                    |
| ------------------------ | ---------- | ---------------------------------------------- |
| `prover_minio_host`      | `minio`    | MinIO hostname (Docker service name)           |
| `prover_minio_bucket`    | `workflow` | S3 bucket name                                 |
| `prover_minio_root_user` | `admin`    | MinIO root user                                |
| `prover_minio_root_pass` | `password` | MinIO root password (**change in production**) |

### Prover Agent Configuration

| Variable                  | Default                                    | Description                     |
| ------------------------- | ------------------------------------------ | ------------------------------- |
| `prover_rust_log`         | `info,broker=debug,boundless_market=debug` | Rust log level                  |
| `prover_risc0_home`       | `/usr/local/risc0`                         | RISC0 home directory            |
| `prover_segment_size`     | `20`                                       | Segment size for proving        |
| `prover_risc0_keccak_po2` | `17`                                       | Keccak power of 2               |
| `prover_redis_ttl`        | `57600`                                    | Redis TTL in seconds (16 hours) |
| `prover_snark_timeout`    | `180`                                      | SNARK timeout in seconds        |

### Binary Configuration

| Variable            | Default            | Description                     |
| ------------------- | ------------------ | ------------------------------- |
| `prover_binary_url` | GitHub release URL | URL to download prover binaries |

### Broker Configuration

These variables enable the broker service:

| Variable                 | Default        | Description                       |
| ------------------------ | -------------- | --------------------------------- |
| `prover_private_key`     | `""`           | Prover wallet private key         |
| `prover_rpc_url`         | `""`           | Single RPC URL (legacy)           |
| `prover_rpc_urls`        | `""`           | RPC URL(s) for broker (preferred) |
| `prover_povw_log_id`     | `""`           | POVW log contract address         |
| `prover_broker_toml_url` | GitHub raw URL | URL to broker.toml template       |

## Docker Role Variables

| Variable                | Default         | Description                     |
| ----------------------- | --------------- | ------------------------------- |
| `docker_ubuntu_version` | (auto-detected) | Ubuntu version override         |
| `docker_architecture`   | `amd64`         | CPU architecture                |
| `docker_users`          | `[]`            | Users to add to docker group    |
| `docker_nvidia_enabled` | `true`          | Enable NVIDIA Container Toolkit |

## NVIDIA Role Variables

| Variable                      | Default         | Description               |
| ----------------------------- | --------------- | ------------------------- |
| `nvidia_cuda_version`         | `13-1`          | CUDA version to install   |
| `nvidia_ubuntu_version`       | (auto-detected) | Ubuntu version override   |
| `nvidia_reboot_after_install` | `false`         | Reboot after installation |

## Vector Role Variables

### Service Configuration

| Variable                 | Default   | Description    |
| ------------------------ | --------- | -------------- |
| `vector_install`         | `true`    | Install Vector |
| `vector_service_enabled` | `true`    | Enable at boot |
| `vector_service_state`   | `started` | Service state  |

### CloudWatch Configuration

| Variable                        | Default                     | Description         |
| ------------------------------- | --------------------------- | ------------------- |
| `vector_cloudwatch_log_group`   | `/boundless/bento/hostname` | Log group name      |
| `vector_cloudwatch_region`      | `us-west-2`                 | AWS region          |
| `vector_cloudwatch_stream_name` | `%Y-%m-%d`                  | Stream name pattern |

### AWS Authentication

| Variable                          | Default | Description                  |
| --------------------------------- | ------- | ---------------------------- |
| `vector_aws_use_instance_profile` | `true`  | Use IAM instance profile     |
| `vector_aws_credentials_file`     | `null`  | Path to AWS credentials file |
| `vector_aws_access_key_id`        | `null`  | AWS access key ID            |
| `vector_aws_secret_access_key`    | `null`  | AWS secret access key        |

### Monitoring and Metrics

| Variable                         | Default                            | Description                              |
| -------------------------------- | ---------------------------------- | ---------------------------------------- |
| `vector_monitor_services`        | `["bento.service"]`                | Systemd units to ingest from journald    |
| `vector_host_metrics_collectors` | `["memory", "disk", "filesystem"]` | Enabled host metrics collectors          |
| `vector_metrics_scrape_interval` | `15`                               | Host metrics scrape interval (seconds)   |
| `vector_track_bento_process`     | `true`                             | Emit `bento_active` and container gauges |
| `vector_track_log_errors`        | `true`                             | Emit `bento_log_errors` metric           |

## AWS CLI Role Variables

| Variable                | Default              | Description                            |
| ----------------------- | -------------------- | -------------------------------------- |
| `awscli_install_method` | `installer`          | Installation method (installer or apt) |
| `awscli_install_dir`    | `/usr/local/aws-cli` | Installation directory                 |
| `awscli_bin_dir`        | `/usr/local/bin`     | Binary directory                       |
| `awscli_update`         | `false`              | Update existing installation           |

## UFW Role Variables

Used by `security.yml` and the `ufw` role:

| Variable                 | Default  | Description                                                     |
| ------------------------ | -------- | --------------------------------------------------------------- |
| `ufw_docker_nat_enabled` | `true`   | If true, add Docker NAT rules when Docker has `iptables: false` |
| `ufw_allowed_ports`      | (varies) | List of `{ port, proto, comment }` for UFW allow rules          |

See `roles/ufw/README.md` and `roles/ufw/defaults/main.yml` for full UFW options.

## Example Configurations

### All-group and staging/production vars (current style)

```yaml
all:
  vars:
    ansible_user: ubuntu
    prover_version: main
    ufw_docker_nat_enabled: true
  children:
    production:
      vars:
        vector_aws_access_key_id: "AKIA..."
        vector_aws_secret_access_key: "..."
      children:
        prover_8453_production_release:
    prover_8453_production_release:
      hosts:
        10.0.0.2:
          prover_postgres_password: "{{ vault_postgres_password }}"
          prover_minio_root_pass: "{{ vault_minio_password }}"
          prover_private_key: "{{ vault_prover_key }}"
          prover_povw_log_id: "0x..."
          prover_rpc_urls: "https://mainnet.example.com/..."
```

### Nightly-style host (build from source)

```yaml
prover_84532_staging_nightly:
  hosts:
    dev-prover:
      prover_boundless_build: "all"
      prover_version: main
      prover_postgres_password: "dev_password"
      prover_minio_root_pass: "dev_password"
      prover_rpc_urls: "https://..."
      prover_broker_toml_url: "https://raw.githubusercontent.com/.../staging_84532.toml"
```

### Vector logging (credentials from group vars)

Set `vector_aws_access_key_id` and `vector_aws_secret_access_key` on the parent group (`staging` or `production`) so all hosts in that environment inherit them. Override per host only if needed.

## Security Best Practices

1. **Never commit plain-text secrets** to git
   - Use base64-encoded inventory in CI/CD secrets
   - Use Ansible Vault for local development

2. **Use strong passwords** for all services
   - `prover_postgres_password`
   - `prover_minio_root_pass`

3. **Protect private keys**
   - `prover_private_key` should be stored securely
   - Consider using hardware wallets or key management systems

4. **Use IAM roles** for AWS credentials when possible
   - Avoid storing AWS credentials in inventory files
   - Use EC2 instance profiles instead
