# Configuration Variables

This document describes all configuration variables for the Boundless Ansible deployment.

## Variable Precedence

Ansible variable precedence (highest to lowest):

1. **Command-line variables** (`-e var=value`)
2. **Inventory host variables** (in `inventory.yml`)
3. **Role defaults** (`roles/*/defaults/main.yml`)

## Quick Start

### Using Inventory Variables

Set variables directly in your inventory file:

```yaml
all:
  children:
    nightly:
      hosts:
        127.0.0.1:
          ansible_user: ubuntu
          prover_version: main
          prover_postgres_password: "secure_password"
          prover_minio_root_pass: "secure_password"
          prover_private_key: "0x..."
          prover_povw_log_id: "0x..."
          prover_rpc_url: "https://..."
```

### Using Command-Line Variables

```bash
ansible-playbook -i inventory.yml prover.yml \
  -e prover_postgres_password="secure_password" \
  -e prover_private_key="0x..."
```

## Prover Role Variables

### Deployment Configuration

| Variable         | Default      | Description                        |
| ---------------- | ------------ | ---------------------------------- |
| `prover_dir`     | `/opt/bento` | Directory where prover is deployed |
| `prover_version` | `v1.2.0`     | Git tag or branch to deploy        |
| `prover_state`   | `started`    | Service state (started, stopped)   |
| `prover_user`    | `ubuntu`     | User for Docker commands           |

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

| Variable                  | Default            | Description                     |
| ------------------------- | ------------------ | ------------------------------- |
| `prover_rust_log`         | `info`             | Rust log level                  |
| `prover_risc0_home`       | `/usr/local/risc0` | RISC0 home directory            |
| `prover_segment_size`     | `20`               | Segment size for proving        |
| `prover_risc0_keccak_po2` | `17`               | Keccak power of 2               |
| `prover_redis_ttl`        | `57600`            | Redis TTL in seconds (16 hours) |
| `prover_snark_timeout`    | `180`              | SNARK timeout in seconds        |

### Binary Configuration

| Variable            | Default            | Description                     |
| ------------------- | ------------------ | ------------------------------- |
| `prover_binary_url` | GitHub release URL | URL to download prover binaries |

### Broker Configuration

These variables enable the broker service:

| Variable                 | Default        | Description                 |
| ------------------------ | -------------- | --------------------------- |
| `prover_private_key`     | `""`           | Prover wallet private key   |
| `prover_rpc_url`         | `""`           | Blockchain RPC URL          |
| `prover_povw_log_id`     | `""`           | POVW log contract address   |
| `prover_broker_toml_url` | GitHub raw URL | URL to broker.toml template |

## Docker Role Variables

| Variable                | Default         | Description                     |
| ----------------------- | --------------- | ------------------------------- |
| `docker_ubuntu_version` | (auto-detected) | Ubuntu version override         |
| `docker_architecture`   | `amd64`         | CPU architecture                |
| `docker_users`          | `[]`            | Users to add to docker group    |
| `docker_nvidia_enabled` | `false`         | Enable NVIDIA Container Toolkit |

## NVIDIA Role Variables

| Variable                      | Default         | Description               |
| ----------------------------- | --------------- | ------------------------- |
| `nvidia_cuda_version`         | `13-0`          | CUDA version to install   |
| `nvidia_ubuntu_version`       | (auto-detected) | Ubuntu version override   |
| `nvidia_reboot_after_install` | `false`         | Reboot after installation |

## Vector Role Variables

### Service Configuration

| Variable                 | Default   | Description    |
| ------------------------ | --------- | -------------- |
| `vector_install`         | `true`    | Install Vector |
| `vector_service_enabled` | `false`   | Enable at boot |
| `vector_service_state`   | `stopped` | Service state  |

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

## AWS CLI Role Variables

| Variable                | Default              | Description                            |
| ----------------------- | -------------------- | -------------------------------------- |
| `awscli_install_method` | `installer`          | Installation method (installer or apt) |
| `awscli_install_dir`    | `/usr/local/aws-cli` | Installation directory                 |
| `awscli_bin_dir`        | `/usr/local/bin`     | Binary directory                       |
| `awscli_update`         | `false`              | Update existing installation           |

## Example Configurations

### Minimal Production Setup

```yaml
all:
  hosts:
    prover-1:
      ansible_user: ubuntu
      prover_version: v1.2.0
      prover_postgres_password: "{{ vault_postgres_password }}"
      prover_minio_root_pass: "{{ vault_minio_password }}"
      prover_private_key: "{{ vault_prover_key }}"
      prover_povw_log_id: "0x..."
      prover_rpc_url: "https://mainnet.infura.io/v3/..."
```

### Nightly Build Setup

```yaml
all:
  children:
    nightly:
      hosts:
        dev-prover:
          ansible_user: ubuntu
          prover_version: main
          prover_postgres_password: "dev_password"
          prover_minio_root_pass: "dev_password"
```

### With Vector Logging

```yaml
all:
  hosts:
    prover-1:
      ansible_user: ubuntu
      prover_version: v1.2.0
      # Vector configuration
      vector_service_enabled: true
      vector_service_state: started
      vector_cloudwatch_log_group: "/boundless/production"
      vector_aws_access_key_id: "AKIA..."
      vector_aws_secret_access_key: "..."
```

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
