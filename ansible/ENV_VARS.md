# Environment Variables for Secrets

This Ansible setup supports sourcing secrets from environment variables, making it easy to use temporary contexts, CI/CD pipelines, and secret management systems.

## Quick Start

Set environment variables before running playbooks:

```bash
export POSTGRESQL_PASSWORD="secure_password"
export MINIO_ROOT_PASSWORD="secure_password"
export BROKER_PRIVATE_KEY="0x..."
export BROKER_RPC_URL="https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY"

ansible-playbook -i inventory.yml broker.yml
```

## Supported Environment Variables

### PostgreSQL

- `POSTGRESQL_USER` (default: `bento`)
- `POSTGRESQL_PASSWORD` (default: `CHANGE_ME` - **must be set**)
- `POSTGRESQL_HOST` (default: `localhost`)
- `POSTGRESQL_PORT` (default: `5432`)
- `POSTGRESQL_DATABASE` (default: `bento`)

### MinIO

- `MINIO_ROOT_USER` (default: `minioadmin`)
- `MINIO_ROOT_PASSWORD` (default: `minioadmin` - **should be changed**)
- `BENTO_S3_ACCESS_KEY` (default: uses `MINIO_ROOT_USER`): MinIO root user (synced with Bento S3 credentials)
- `BENTO_S3_SECRET_KEY` (default: uses `MINIO_ROOT_PASSWORD`): MinIO root password (synced with Bento S3 credentials)

### Broker

- `BROKER_BENTO_API_URL` (default: `http://localhost:8081`): Bento REST API URL
- `BROKER_RPC_URL` or `RPC_URL` (default: `https://rpc.boundless.network` - **should be overridden**)
- `BROKER_PRIVATE_KEY` or `PRIVATE_KEY` (default: zero key - **must be set**)
- `BROKER_MIN_MCYCLE_PRICE` (default: `0.0000003`): Minimum price per mega-cycle
- `BROKER_MIN_MCYCLE_PRICE_COLLATERAL_TOKEN` (default: `0`): Minimum price in collateral token
- `BROKER_PEAK_PROVE_KHZ` (default: `100`): Estimated peak performance in kHz
- `BROKER_MAX_COLLATERAL` (default: `200`): Maximum collateral amount
- `BROKER_MAX_CONCURRENT_PREFLIGHTS` (default: `4`, auto-calculated from CPU): Maximum concurrent preflight tasks
- `BROKER_MAX_CONCURRENT_PROOFS` (default: `1`): Maximum concurrent proof tasks
- `PROMETHEUS_METRICS_ADDR` (default: `127.0.0.1:9090`): Prometheus metrics endpoint (shared by all services)
- `BROKER_PROMETHEUS_METRICS_ADDR` (default: uses `PROMETHEUS_METRICS_ADDR`): Broker-specific metrics endpoint

### Bento

- `BENTO_S3_BUCKET` (default: `bento`)
- `BENTO_S3_ACCESS_KEY` (default: uses `MINIO_ROOT_USER`)
- `BENTO_S3_SECRET_KEY` (default: uses `MINIO_ROOT_PASSWORD`)
- `BENTO_S3_URL` (default: uses MinIO host/port)
- `BENTO_S3_REGION` (default: `auto`)
- `BENTO_REWARDS_ADDRESS` (default: empty)
- `BENTO_POVW_LOG_ID` (default: empty, can also use `POVW_LOG_ID`)
- `SEGMENT_PO2` (default: `20`): Segment size parameter
- `KECCAK_PO2` (default: `17`): Keccak size parameter
- `BENTO_GPU_COUNT` (default: `1`): Number of GPU prove workers
- `BENTO_EXEC_COUNT` (default: `4`): Number of exec workers
- `BENTO_AUX_COUNT` (default: `2`): Number of aux workers
- `PROMETHEUS_METRICS_ADDR` (default: `127.0.0.1:9090`): Prometheus metrics endpoint (shared by all services)
- `BENTO_PROMETHEUS_METRICS_ADDR` (default: uses `PROMETHEUS_METRICS_ADDR`): Bento-specific metrics endpoint

## Usage Examples

### Using a .env file

Create `.env` file:

```bash
# .env
export POSTGRESQL_PASSWORD="secure_password_123"
export MINIO_ROOT_PASSWORD="secure_minio_password"
export BROKER_PRIVATE_KEY="0x1234567890abcdef..."
export BROKER_RPC_URL="https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY"
```

Source it before running:

```bash
source .env
ansible-playbook -i inventory.yml broker.yml
```

### Using direnv

Create `.envrc`:

```bash
# .envrc
export POSTGRESQL_PASSWORD="secure_password_123"
export MINIO_ROOT_PASSWORD="secure_minio_password"
export BROKER_PRIVATE_KEY="0x1234567890abcdef..."
export BROKER_RPC_URL="https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY"
```

Then:

```bash
direnv allow
ansible-playbook -i inventory.yml broker.yml
```

### Using AWS Secrets Manager

```bash
# Fetch secrets from AWS Secrets Manager
export POSTGRESQL_PASSWORD=$(aws secretsmanager get-secret-value \
  --secret-id boundless/postgresql/password \
  --query SecretString --output text)

export BROKER_PRIVATE_KEY=$(aws secretsmanager get-secret-value \
  --secret-id boundless/broker/private-key \
  --query SecretString --output text)

ansible-playbook -i inventory.yml broker.yml
```

### Using Bitwarden CLI

```bash
# Unlock Bitwarden
export BW_SESSION=$(bw unlock --raw)

# Fetch secrets
export POSTGRESQL_PASSWORD=$(bw get password "PostgreSQL - Boundless")
export BROKER_PRIVATE_KEY=$(bw get password "Broker Private Key")

ansible-playbook -i inventory.yml broker.yml
```

### Using HashiCorp Vault

```bash
# Authenticate with Vault
export VAULT_TOKEN=$(vault login -method=userpass username=myuser -format=json | jq -r .auth.client_token)

# Fetch secrets
export POSTGRESQL_PASSWORD=$(vault kv get -field=password secret/boundless/postgresql)
export BROKER_PRIVATE_KEY=$(vault kv get -field=private_key secret/boundless/broker)

ansible-playbook -i inventory.yml broker.yml
```

### Using 1Password CLI

```bash
# Authenticate
eval $(op signin)

# Fetch secrets
export POSTGRESQL_PASSWORD=$(op read "op://Boundless/PostgreSQL/password")
export BROKER_PRIVATE_KEY=$(op read "op://Boundless/Broker/private-key")

ansible-playbook -i inventory.yml broker.yml
```

## CI/CD Integration

### GitHub Actions

```yaml
- name: Run Ansible playbook
  env:
    POSTGRESQL_PASSWORD: ${{ secrets.POSTGRESQL_PASSWORD }}
    BROKER_PRIVATE_KEY: ${{ secrets.BROKER_PRIVATE_KEY }}
    BROKER_RPC_URL: ${{ secrets.BROKER_RPC_URL }}
  run: |
    ansible-playbook -i inventory.yml broker.yml
```

### GitLab CI

```yaml
deploy:
  script:
    - export POSTGRESQL_PASSWORD="$POSTGRESQL_PASSWORD"
    - export BROKER_PRIVATE_KEY="$BROKER_PRIVATE_KEY"
    - ansible-playbook -i inventory.yml broker.yml
  variables:
    POSTGRESQL_PASSWORD: $POSTGRESQL_PASSWORD
    BROKER_PRIVATE_KEY: $BROKER_PRIVATE_KEY
```

### Jenkins

```groovy
pipeline {
    agent any
    environment {
        POSTGRESQL_PASSWORD = credentials('postgresql-password')
        BROKER_PRIVATE_KEY = credentials('broker-private-key')
    }
    stages {
        stage('Deploy') {
            steps {
                sh 'ansible-playbook -i inventory.yml broker.yml'
            }
        }
    }
}
```

## Variable Precedence

Ansible variable precedence (highest to lowest):

1. **Command line variables** (`-e var=value`)
2. **Environment variables** (via `lookup('env', 'VAR')`)
3. **Playbook variables** (`vars:` in playbook)
4. **Inventory variables** (`host_vars/`, `group_vars/`)
5. **Role defaults** (what we've set up)

This means environment variables will override defaults but can be overridden by command-line or playbook variables.

## Security Best Practices

1. **Never commit `.env` files to git**
   - Add `.env` to `.gitignore`
   - Use `.env.example` as a template

2. **Use secure secret management**
   - AWS Secrets Manager
   - HashiCorp Vault
   - Bitwarden
   - 1Password
   - CI/CD secret stores

3. **Clear environment after use**
   ```bash
   unset POSTGRESQL_PASSWORD
   unset BROKER_PRIVATE_KEY
   ```

4. **Use separate contexts**
   - Different `.env` files for different environments
   - Use `direnv` for automatic context switching

5. **Rotate secrets regularly**
   - Update environment variables when rotating secrets
   - No need to re-encrypt vault files

## Example: Complete Setup Script

Create `scripts/load-secrets.sh`:

```bash
#!/bin/bash
# Load secrets from your preferred secret manager

set -e

# Option 1: Load from .env file
if [ -f .env ]; then
    source .env
fi

# Option 2: Load from AWS Secrets Manager
if command -v aws &> /dev/null; then
    export POSTGRESQL_PASSWORD=$(aws secretsmanager get-secret-value \
        --secret-id boundless/postgresql/password \
        --query SecretString --output text 2>/dev/null || echo "")
fi

# Option 3: Load from Bitwarden
if command -v bw &> /dev/null && [ -n "$BW_SESSION" ]; then
    export BROKER_PRIVATE_KEY=$(bw get password "Broker Private Key" 2>/dev/null || echo "")
fi

# Validate required secrets
if [ -z "$POSTGRESQL_PASSWORD" ] || [ "$POSTGRESQL_PASSWORD" = "CHANGE_ME" ]; then
    echo "ERROR: POSTGRESQL_PASSWORD must be set"
    exit 1
fi

echo "Secrets loaded successfully"
```

Then use it:

```bash
source scripts/load-secrets.sh
ansible-playbook -i inventory.yml broker.yml
```

## Troubleshooting

### "Variable is empty"

- Check that the environment variable is set: `echo $POSTGRESQL_PASSWORD`
- Verify variable name matches exactly (case-sensitive)
- Check if variable is exported: `export POSTGRESQL_PASSWORD="value"`

### "Default value used instead of env var"

- Ensure variable is exported, not just set: `export VAR=value`
- Check variable precedence (command-line vars override env vars)

### "Secrets not loading in CI/CD"

- Verify secrets are set in CI/CD secret store
- Check that secrets are exported in the CI/CD environment
- Use `ansible-playbook ... -e "var=value"` to pass secrets directly
