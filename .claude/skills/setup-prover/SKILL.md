---
name: setup-prover
description: >-
  Set up and deploy a Boundless prover to a GPU server using Ansible. Handles inventory
  setup, SSH connectivity, NVIDIA drivers, Docker, and the full bento stack.
  Use when deploying a new prover, redeploying to an existing server, or
  troubleshooting prover infrastructure.
---

# Deploy Prover

Interactive helper for deploying the Boundless prover stack to GPU servers.

## What you can help with

1. **New prover deployment** — create inventory, deploy full stack (NVIDIA + Docker + bento)
2. **Redeploy/update** — update images, config, or git version on an existing server
3. **Tear down** — stop services and clean up
4. **Troubleshoot** — check service health, logs, GPU status
5. **Configure** — tune broker.toml settings (peak_prove_khz, concurrency, etc.)

## Prerequisites

Before deploying, verify the user has:

1. **Ansible** — `ansible --version` (install: `brew install ansible`)
2. **SSH access** — `ssh <user>@<host> "echo connected"`
3. **GPU server** — Ubuntu 22.04 or 24.04 with NVIDIA GPU hardware

## Deployment workflow

### 1. Create inventory file

Create `ansible/inventory-<name>.yml` with the server details. NEVER hardcode secrets — use env var lookups:

```yaml
---
all:
  vars:
    ansible_user: ubuntu
    ansible_ssh_private_key_file: ~/.ssh/<key>
    # ansible_become_password: "{{ lookup('env', 'ANSIBLE_BECOME_PASSWORD') }}"  # uncomment if server needs sudo password
    prover_version: main  # or a specific branch/tag
  children:
    <group_name>:
      hosts:
        <ip_address>:
          prover_postgres_password: "<password>"
          prover_minio_root_pass: "<password>"
          prover_image_tag: "nightly-latest"
          prover_boundless_build: ""
          prover_rpc_urls: "{{ lookup('env', 'PROVER_RPC_URLS') }}"
          prover_private_key: "{{ lookup('env', 'PROVER_PRIVATE_KEY') }}"
          # prover_executor_count and max_concurrent_preflights default to 8 (matching broker-template.toml)
          prover_broker_toml_url: "https://raw.githubusercontent.com/boundless-xyz/boundless/refs/heads/main/broker-template.toml"
          # prover_broker_toml_local: "/path/to/custom/broker.toml"  # uncomment to use a local file instead
```

Ensure the inventory file is in `.gitignore` (`ansible/inventory-*.yml` pattern).

### 2. Set environment variables

The inventory uses `{{ lookup('env', 'VAR') }}` for secrets. These resolve on the machine running `ansible-playbook`, not on the remote server. They must be exported in the same shell that runs the playbook.

```bash
export PROVER_PRIVATE_KEY="<broker private key>"
export PROVER_RPC_URL_8453="<Base RPC URL>"
export PROVER_RPC_URL_167000="<Taiko RPC URL>"
ansible-playbook -i inventory-<name>.yml prover.yml  # must be in the same shell
```

### 3. Test connectivity

```bash
cd ansible
ansible all -i inventory-<name>.yml -m ping
```

### 4. Deploy

```bash
PROVER_PRIVATE_KEY="$PROVER_PRIVATE_KEY" \
PROVER_RPC_URLS="$PROVER_RPC_URLS" \
ansible-playbook -i inventory-<name>.yml prover.yml
```

This installs: NVIDIA drivers, Docker + NVIDIA Container Toolkit, then deploys the bento compose stack.

### 5. Verify

```bash
# Check all services are running
ssh <user>@<host> "sudo docker compose -f /opt/bento/compose.yml --env-file /opt/bento/.env ps"

# Check GPU is detected
ssh <user>@<host> "nvidia-smi --query-gpu=name,memory.total --format=csv,noheader"

# Check broker logs
ssh <user>@<host> "sudo docker compose -f /opt/bento/compose.yml --env-file /opt/bento/.env logs broker --tail 20"

# Check all images are correct
ssh <user>@<host> "sudo docker compose -f /opt/bento/compose.yml --env-file /opt/bento/.env ps --format '{{.Name}}\t{{.Image}}'"
```

## Image modes

Before generating the inventory, ask the user which image mode they want. This matters because building from source is slow (Rust compilation) but required when deploying code from a feature branch that doesn't have CI-built images.

Ask: **"Do you want to use pre-built images or build from source? If building from source, which components?"**

| Mode                      | Config                                | Description                                                                                       |
| ------------------------- | ------------------------------------- | ------------------------------------------------------------------------------------------------- |
| **Pre-built nightly**     | `prover_image_tag: "nightly-latest"`  | Pull latest CI images from GHCR (recommended for `main` branch)                                   |
| **Pinned nightly SHA**    | `prover_image_tag: "nightly-abc1234"` | Pin to a specific CI build by commit SHA                                                          |
| **Custom tag**            | `prover_image_tag: "my-custom-tag"`   | Any tag from GHCR (e.g. from `gh workflow run docker-services.yml -f custom_tag="my-custom-tag"`) |
| **Pinned release**        | `prover_image_version: "1.3"`         | Use release-tagged images (e.g. `bento-v1.3`, `broker-v1.3`)                                      |
| **Build all from source** | `prover_boundless_build: "all"`       | Build all Docker images on the server (slow, not recommended unless needed)                       |
| **Selective build**       | `prover_boundless_build: "broker"`    | Build specific service(s) from source, pull pre-built images for the rest                         |

`prover_image_tag` takes precedence over `prover_image_version`. When set, ALL images use the exact same tag.

### Selective build + pre-built (hybrid mode)

You can combine `prover_image_tag` with `prover_boundless_build` to build only specific services from source while pulling pre-built images for everything else. The justfile handles this by clearing the image tag for the built service at runtime (`export BROKER_IMAGE=""`) so docker compose builds it from the local Dockerfile instead of pulling.

Example: deploy a feature branch that only changes the broker:

```yaml
prover_image_tag: "nightly-latest"       # pre-built images for agents, rest_api, etc.
prover_boundless_build: "broker"          # build only the broker from the branch source
```

Valid `prover_boundless_build` values: `"all"`, `"broker"`, `"rest_api"`, `"exec_agent"`, `"aux_agent"`, `"gpu_prove_agent"`, `"miner"`, or space-separated combinations (e.g. `"broker rest_api"`).

## Broker configuration

Ask the user: **"Do you want to use an existing broker config from the repo, or specify your own config directory?"**

**Option A: Use an existing config.** List the directories in `ansible/roles/prover/configs/broker/` and let the user pick:

```yaml
prover_broker_config_dir: "roles/prover/configs/broker/<chosen-config>"
```

**Option B: Specify a custom directory.** The user provides a path to a directory containing their own config files:

```yaml
prover_broker_config_dir: "/path/to/my/broker-config"
```

Ansible copies the entire directory contents to `/opt/bento/` on the remote server.

The config directory should contain:

- `broker.toml` — base config shared across all chains
- `broker.{chain_id}.toml` — per-chain overrides (e.g. `broker.8453.toml`, `broker.167000.toml`)

Per-chain override files must go in a `chain-overrides/` subdirectory (e.g. `chain-overrides/broker.8453.toml`). This is the directory mounted into the Docker container.

IMPORTANT: `broker.toml` must include `min_mcycle_price` and `max_collateral` even if per-chain overrides replace them. These are required fields — the broker fails to parse the base config without them.

Fallback options (when `prover_broker_config_dir` is not set):

- `prover_broker_toml_local` — copies a single local file to `/opt/bento/broker.toml`
- `prover_broker_toml_url` — downloads from a URL (default: `broker-template.toml` from main branch)

### Auto-generating config with the CLI wizard

The `boundless` CLI has a wizard that generates optimized configs:

```bash
# Run locally — auto-detects local hardware, or you can manually enter the server's specs
boundless prover generate-config --broker-toml-file ./broker-<host>.toml

# Then use it in the inventory:
# prover_broker_toml_local: "./broker-<host>.toml"
```

The wizard calculates `peak_prove_khz`, `max_concurrent_preflights`, `max_exec_agents`, and other values. If run on a machine without GPUs (e.g. your Mac), it will prompt you to enter the values manually.

Key settings to tune per GPU:

| Setting                     | Default | Description                                                   |
| --------------------------- | ------- | ------------------------------------------------------------- |
| `peak_prove_khz`            | 100     | Estimated GPU proving throughput. Too low = underutilizes GPU |
| `max_concurrent_preflights` | 8       | Should match `prover_executor_count`                          |
| `max_concurrent_proofs`     | 1       | Number of concurrent GPU proofs                               |

## Selective deployment

Use `--skip-tags` to skip specific roles:

```bash
# Skip NVIDIA driver installation
ansible-playbook -i inventory-<name>.yml prover.yml --skip-tags nvidia

# Skip both NVIDIA and Docker (host already has them)
ansible-playbook -i inventory-<name>.yml prover.yml --skip-tags nvidia,docker
```

Note: `--tags prover` does NOT work as expected — the individual tasks inside the role are not tagged. Use `--skip-tags` instead. IMPORTANT: `--skip-tags nvidia,docker` will ALSO skip the prover role because it is tagged with both `prover` and `docker`. To skip only the NVIDIA and Docker install roles, use `--skip-tags nvidia-install,docker-install`. The NVIDIA/Docker roles are idempotent so running the full playbook on an already-configured host is safe and fast.

## Tear down

```bash
# Stop all services
ssh <user>@<host> "sudo docker compose -f /opt/bento/compose.yml --env-file /opt/bento/.env down"

# Or stop via systemd
ssh <user>@<host> "sudo systemctl stop bento"
```

## Troubleshooting

### SSH permission denied

Check which SSH key the server expects:

```bash
ssh -v <user>@<host> 2>&1 | grep "Offering\|Accepted"
```

Add `ansible_ssh_private_key_file` to the inventory.

### NVIDIA drivers not detected after deploy

Reboot the server: `ssh <user>@<host> "sudo reboot"`

### Services crash-looping

Check logs: `ssh <user>@<host> "sudo docker compose -f /opt/bento/compose.yml --env-file /opt/bento/.env logs --tail 50"`

### Postgres password mismatch after redeployment

If you change `prover_postgres_password` on a server that already has a running postgres, the old password persists in the Docker volume. Fix:

```bash
ssh <user>@<host> "sudo docker compose -f /opt/bento/compose.yml --env-file /opt/bento/.env down -v"
ssh <user>@<host> "sudo systemctl start bento"
```

Warning: `-v` deletes all volumes including broker DB and MinIO data.

### Sudo password required

If ansible fails with "Missing sudo password", add to inventory vars:

```yaml
ansible_become_password: "{{ lookup('env', 'ANSIBLE_BECOME_PASSWORD') }}"
```

Then export before running: `export ANSIBLE_BECOME_PASSWORD="<password>"`

Cloud providers (Latitude.sh, AWS) typically have passwordless sudo. Local/dev machines often don't.

### Broker not picking up orders

1. Check RPC URL is correct and accessible
2. Check broker.toml has correct contract addresses
3. Check broker logs for error codes

## Deploying a branch to staging

Two options for deploying a feature branch instead of `main`:

### Option 1: Via CI pipeline (inventory override)

The pipeline inventory is in AWS Secrets Manager (`l-prover-ansible-inventory`, base64-encoded YAML). Override `prover_version` for staging hosts:

```bash
# Decode current inventory
aws secretsmanager get-secret-value --secret-id l-prover-ansible-inventory --region us-west-2 \
  --query SecretString --output text | base64 -D > inventory.yml

# Edit: set prover_version to your branch for staging hosts
# Then update the secret:
aws secretsmanager put-secret-value \
  --secret-id l-prover-ansible-inventory \
  --secret-string "$(base64 -i inventory.yml | tr -d '\n')" \
  --region us-west-2
```

Then run the pipeline (push to main or manual execution in CodePipeline).

To also use broker config from the branch, set `prover_broker_toml_url` to point at the branch's raw file.

**Revert when done**: set `prover_version` back to `main`, update the secret, re-run pipeline.

### Option 2: Local ansible (no pipeline)

```bash
cd ansible
ansible-playbook -i inventory.yml prover.yml --limit staging -e prover_version=feature/my-branch

# With broker config from the same branch:
ansible-playbook -i inventory.yml prover.yml --limit staging \
  -e prover_version=feature/my-branch \
  -e "prover_broker_toml_url=https://raw.githubusercontent.com/boundless-xyz/boundless/refs/heads/feature/my-branch/broker-template.toml"
```

### Skip NVIDIA/Docker (host already set up)

```bash
ansible-playbook -i inventory.yml prover.yml --limit staging --skip-tags nvidia,docker -e prover_version=feature/my-branch
```

## Bento worker overflow (production burst fleet)

The `bento-worker-overflow` ASG in the production account is a manually-scaled GPU burst fleet.

```bash
# Check current state
aws-vault exec boundless-prod
aws autoscaling describe-auto-scaling-groups \
  --auto-scaling-group-names bento-worker-overflow \
  --region us-west-2 \
  --query 'AutoScalingGroups[0].{Desired:DesiredCapacity,Min:MinSize,Max:MaxSize,Instances:length(Instances)}' \
  --output table

# Scale to N instances
aws autoscaling update-auto-scaling-group \
  --auto-scaling-group-name bento-worker-overflow \
  --desired-capacity 4 --region us-west-2

# Drain (scale to zero)
aws autoscaling update-auto-scaling-group \
  --auto-scaling-group-name bento-worker-overflow \
  --desired-capacity 0 --region us-west-2
```

### When to scale

| Queue Depth | Suggested Overflow     |
| ----------- | ---------------------- |
| < 20        | 0 (primary handles it) |
| 20-50       | 2-4                    |
| 50-200      | 4-8                    |
| 200+        | 8+ (check ASG max)     |

Scale down gradually (8 → 4 → 0) so in-flight proofs can finish. Wait until queue is stable/near-zero for 30+ minutes.

### Check broker queue depth

```bash
LOG_GROUP="/boundless/bento/YOUR_HOSTNAME"
START=$(date -v-24H +%s)
END=$(date +%s)
Q=$(aws logs start-query --log-group-name "$LOG_GROUP" --start-time $START --end-time $END \
  --query-string 'fields @timestamp, @message | filter @message like /.* orders ready to be locked .*/ | sort @timestamp desc | limit 1' \
  --region us-west-2 --output text --query 'queryId')
until [ "$(aws logs get-query-results --query-id $Q --region us-west-2 --output text --query 'status')" = "Complete" ]; do sleep 1; done
aws logs get-query-results --query-id $Q --region us-west-2 --output json | jq -r '.results[0][] | select(.field == "@message") | .value'
```

## Key references

| What                | Location                                         |
| ------------------- | ------------------------------------------------ |
| Pipeline definition | `infra/pipelines/pipelines/l-prover-ansible.ts`  |
| Pipeline inventory  | AWS Secrets Manager `l-prover-ansible-inventory` |
| Prover role         | `ansible/roles/prover/tasks/main.yml`            |
| Role defaults       | `ansible/roles/prover/defaults/main.yml`         |
| All variables       | `ansible/ENV_VARS.md`                            |
| Inventory guide     | `ansible/INVENTORY.md`                           |

## Supported providers

Tested with:

- **Latitude.sh** — bare metal and GPU VMs, `ubuntu` user, SSH key via their CLI (`lsh`)
- **AWS** — EC2 GPU instances
- **Local dev machine** — Ubuntu 24.04 with RTX 5090 (NVIDIA/Docker roles are idempotent and skip if already installed)

Any Ubuntu 22.04/24.04 server with NVIDIA GPU and SSH access will work.

## IMPORTANT

- NEVER hardcode private keys or RPC URLs in inventory files — use `{{ lookup('env', 'VAR') }}`
- Ensure `ansible/inventory-*.yml` is in `.gitignore`
- The default `broker-template.toml` may not be optimal for your GPU — tune `peak_prove_khz` after deployment
- `prover_executor_count` and `max_concurrent_preflights` should match. Default role has 4 executors but broker-template.toml has 8 preflights — always override one or both
