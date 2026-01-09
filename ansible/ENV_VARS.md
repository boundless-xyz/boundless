# Configuration Variables

This Ansible setup uses Ansible's built-in variable system for configuration management. Variables can be set at multiple levels with clear precedence.

## Variable Precedence

Ansible variable precedence (highest to lowest):

1. **Command-line variables** (`-e var=value`)
2. **Host variables** (`host_vars/HOSTNAME/main.yml` or `host_vars/HOSTNAME/vault.yml`)
3. **Group variables** (`group_vars/all/*.yml`)
4. **Role defaults** (`roles/*/defaults/main.yml`)

## Quick Start

### Using Host Variables (Recommended)

Create host-specific configuration files:

```bash
# Non-sensitive configuration
nano ansible/host_vars/example/main.yml
```

```yaml
# host_vars/example/main.yml
postgresql_host: "10.0.1.10"
valkey_host: "10.0.1.10"
minio_host: "10.0.1.10"
bento_install_dependencies: false
```

For sensitive values, use Ansible Vault:

```bash
# Create encrypted vault file
ansible-vault create ansible/host_vars/example/vault.yml
```

```yaml
# host_vars/example/vault.yml (encrypted)
postgresql_password: "secure_password"
minio_root_user: "s3_access_key"
minio_root_password: "s3_secret_key"
broker_private_key: "0x..."
```

### Using Command-Line Variables

```bash
ansible-playbook -i inventory.yml broker.yml \
  -e postgresql_password="secure_password" \
  -e broker_private_key="0x..."
```

## Configuration Variables

### PostgreSQL

Set in `host_vars` or via `-e`:

- `postgresql_user` (default: `"bento"`)
- `postgresql_password` (default: `"CHANGE_ME"` - **must be set**)
- `postgresql_host` (default: `"localhost"`)
- `postgresql_port` (default: `5432`)
- `postgresql_database` (default: `"bento"`)

### MinIO/S3

Set in `host_vars` or via `-e`:

- `minio_host` (default: `"localhost"`)
- `minio_port` (default: `9000`)
- `minio_root_user` (default: `"minioadmin"`)
- `minio_root_password` (default: `"minioadmin"` - **should be changed**)

Bento S3 configuration automatically uses MinIO credentials:

- `bento_s3_bucket` (default: `"bento"`)
- `bento_s3_url` (default: `http://{{ minio_host }}:{{ minio_port }}`)
- `bento_s3_access_key` (default: `{{ minio_root_user }}`)
- `bento_s3_secret_key` (default: `{{ minio_root_password }}`)
- `bento_s3_region` (default: `"auto"`)

### Valkey (Redis)

Set in `host_vars` or via `-e`:

- `valkey_host` (default: `"localhost"`)
- `valkey_port` (default: `6379`)
- `valkey_maxmemory` (default: `"12gb"`)

### Broker

Set in `host_vars` or via `-e`:

- `broker_bento_api_url` (default: `"http://localhost:8081"`)
- `broker_rpc_url` (default: `""` - **must be set**)
- `broker_private_key` (default: `""` - **must be set**)
- `broker_min_mcycle_price` (default: `"0.00000001"`)
- `broker_min_mcycle_price_collateral_token` (default: `"0.00005"`)
- `broker_peak_prove_khz` (default: `100`)
- `broker_max_collateral` (default: `"200"`)
- `broker_max_concurrent_preflights` (default: `2`)
- `broker_max_concurrent_proofs` (default: `1`)
- `broker_prometheus_metrics_addr` (default: `"127.0.0.1:9590"`)
- `broker_user` (default: `"{{ bento_user | default('bento') }}"`): Service user (defaults to `bento` user)
- `broker_group` (default: `"{{ bento_group | default('bento') }}"`): Service group (defaults to `bento` group)

### Bento

Set in `host_vars` or via `-e`:

- `bento_task` (default: `"prove"`): Legacy tag label (not used by the launcher)
- `bento_count` (default: `1`): Legacy instance count (not used by the launcher)
- `bento_segment_po2` (default: `20`)
- `bento_keccak_po2` (default: `17`)
- `bento_rewards_address` (default: `""`)
- `bento_povw_log_id` (default: `""`)
- `bento_prometheus_metrics_addr` (default: `"127.0.0.1:9090"`): Base Prometheus metrics address (ports vary by service: api=9090, prove=9190, exec=9290, aux=9390, snark=9490, join=9490)
- `bento_version` (default: `"v1.2.0"`): Version of Bento to install (tracked in `/etc/boundless/.bento_version`)
- `bento_gpu_count` (default: `null`): Number of GPU prove workers (null = auto-detect, 0 = disable)
- `bento_exec_count` (default: `4`): Number of exec workers
- `bento_aux_count` (default: `2`): Number of aux workers
- `bento_snark_count` (default: `1`): Number of snark workers (when `bento_snark_workers` is true)
- `bento_join_count` (default: `1`): Number of join workers (when `bento_join_workers` is true)
- `bento_snark_workers` (default: `false`): Enable snark workers
- `bento_join_workers` (default: `false`): Enable join workers
- `bento_enable_api` (default: `true`): Enable the API service

## Remote Worker Node Configuration

For worker nodes connecting to remote services, create `host_vars/HOSTNAME/main.yml`:

```yaml
# Disable local service installation
postgresql_install: false
valkey_install: false
minio_install: false
bento_install_dependencies: false

# Point to remote services
postgresql_host: "10.0.1.10"  # Manager node IP
postgresql_port: 5432
postgresql_user: "bento"
# postgresql_password: Set in vault.yml

valkey_host: "10.0.1.10"
valkey_port: 6379

minio_host: "10.0.1.10"
minio_port: 9000
# minio_root_user: Set in vault.yml
# minio_root_password: Set in vault.yml

# Bento service configuration
bento_enable_api: false
bento_gpu_count: 1
bento_exec_count: 0
bento_aux_count: 0
```

See `host_vars/example/main.yml` for a complete example.

## Ansible Vault

For sensitive values, use Ansible Vault:

```bash
# Create encrypted vault file
ansible-vault create host_vars/example/vault.yml

# Edit existing vault file
ansible-vault edit host_vars/example/vault.yml

# View vault file
ansible-vault view host_vars/example/vault.yml
```

When running playbooks with vault files:

```bash
# Prompt for vault password
ansible-playbook -i inventory.yml cluster.yml --ask-vault-pass

# Or use vault password file
ansible-playbook -i inventory.yml cluster.yml --vault-password-file ~/.vault_pass
```

## CI/CD Integration

### GitHub Actions

```yaml
- name: Run Ansible playbook
  env:
    ANSIBLE_VAULT_PASSWORD: ${{ secrets.VAULT_PASSWORD }}
  run: |
    echo "$ANSIBLE_VAULT_PASSWORD" > .vault_pass
    ansible-playbook -i inventory.yml cluster.yml \
      --vault-password-file .vault_pass \
      -e postgresql_password="${{ secrets.POSTGRESQL_PASSWORD }}" \
      -e broker_private_key="${{ secrets.BROKER_PRIVATE_KEY }}"
```

### GitLab CI

```yaml
deploy:
  script:
    - echo "$VAULT_PASSWORD" > .vault_pass
    - ansible-playbook -i inventory.yml cluster.yml \
        --vault-password-file .vault_pass \
        -e postgresql_password="$POSTGRESQL_PASSWORD" \
        -e broker_private_key="$BROKER_PRIVATE_KEY"
  variables:
    POSTGRESQL_PASSWORD: $POSTGRESQL_PASSWORD
    BROKER_PRIVATE_KEY: $BROKER_PRIVATE_KEY
```

## Security Best Practices

1. **Never commit unencrypted vault files** to git
   - Add `*.vault.yml` to `.gitignore` if not using ansible-vault
   - Use `ansible-vault encrypt` for sensitive files

2. **Use secure secret management**
   - Ansible Vault (built-in)
   - AWS Secrets Manager (fetch and pass via `-e`)
   - HashiCorp Vault (fetch and pass via `-e`)
   - CI/CD secret stores

3. **Separate environments**
   - Different `host_vars` for different environments
   - Use inventory groups for environment separation

4. **Rotate secrets regularly**
   - Update vault files when rotating secrets
   - Re-encrypt vault files after updates

## Troubleshooting

### "Variable is undefined"

- Check variable name spelling (case-sensitive)
- Verify variable is set in the correct precedence level
- Check if variable is defined in role defaults

### "Default value used instead of custom value"

- Check variable precedence (command-line vars override host\_vars)
- Verify variable name matches exactly
- Check for typos in variable names

### "Vault password required"

- Use `--ask-vault-pass` to prompt for password
- Or use `--vault-password-file` with a password file
- Ensure vault password file has correct permissions (`chmod 600`)
