---
name: infra-deploy
description: >-
  Help developers deploy Boundless AWS infrastructure to the dev environment.
  Use when the user wants to deploy a service to dev, bootstrap their dev
  environment, troubleshoot infrastructure issues, or understand what env vars
  a service needs. Also handles teardown and status checks.
---

# Infra Deploy Assistant

Interactive helper for deploying Boundless services to the AWS dev environment.

## What you can help with

1. **First-time setup** — bootstrap a developer's personal VPC and verify with the sample project
2. **Deploy a service** — figure out required env vars, run the deploy
3. **Tear down** — destroy a service or the entire dev environment
4. **Troubleshoot** — diagnose common errors (missing env vars, permissions, stack references)
5. **Explain** — what a service does, what config it needs, how it connects to other services

## Prerequisites check

Before any deploy, verify the user has:

1. **Pulumi CLI** — run `pulumi version`
2. **Node.js** — run `node --version` (needs v18+)
3. **AWS CLI** — run `aws --version`
4. **aws-vault** — run `aws-vault --version`
5. **AWS dev account access** — run `aws-vault exec boundless-dev -- aws sts get-caller-identity`

If any are missing, tell the user how to install them (brew on macOS, or the official install commands).

If the user hasn't set up aws-vault yet, they need to add this to `~/.aws/config`:

```ini
[profile boundless-dev]
sso_start_url = https://risczero.awsapps.com/start
sso_region = us-west-2
sso_account_id = <ask-your-team-lead>
sso_role_name = DeveloperAdmin
region = us-west-2
```

## Key conventions

- **`DEV_NAME`** — a short lowercase identifier for the developer (e.g. "angelo", "jonas"). Used as prefix for all AWS resources. Must be consistent across all deploys.
- **`BASE_STACK`** — Pulumi stack reference to the bootstrap VPC. For dev: `organization/bootstrap/services-dev`
- **Dev mode** — all secrets come from environment variables (via `getEnvVar()`), not Pulumi config.
- **Local Pulumi state** — dev stacks use `pulumi login --local`, not the S3 backend.

## How to find required env vars for a service

Read the service's `infra/<service>/index.ts` and look for:

- `getEnvVar("VAR_NAME")` — required env var in dev mode
- `config.require("VAR_NAME")` — required Pulumi config (staging/prod only)
- `config.requireSecret("VAR_NAME")` — required secret (staging/prod only)

Common env vars across services:

- `DEV_NAME` — always required in dev
- `BASE_STACK` — always required (reference to bootstrap stack)
- `ETH_RPC_URL` or `RPC_URL` — RPC endpoint
- `PRIVATE_KEY` — signer key
- `CHAIN_ID` — target chain

## AWS session

Before any deploy, suggest the user start a persistent AWS session so they don't need to prefix every command with `aws-vault exec`:

```bash
aws-vault exec boundless-dev -s -d 12h
```

This runs a local credential server. All subsequent AWS/Pulumi commands in that shell just work.

## Bootstrap workflow

```bash
cd infra/bootstrap
export DEV_NAME="<name>" PULUMI_CONFIG_PASSPHRASE=""
pulumi login --local
pulumi stack init services-dev
pulumi install
pulumi up
```

This creates: VPC, deployment role, ECR scanning config, alarm tags role.

## Service deploy workflow

```bash
cd infra/<service>
export DEV_NAME="<name>"
export BASE_STACK="organization/bootstrap/services-dev"
export PULUMI_CONFIG_PASSPHRASE=""  # avoids passphrase prompt for dev stacks
# ... set service-specific env vars (read from index.ts)
pulumi login --local
pulumi stack init dev
pulumi install
pulumi up
```

## Docker builds on Apple Silicon (M-series Macs)

Service Docker images target `linux/amd64`. Building locally on Apple Silicon via QEMU is very slow and often fails (OpenSSL, native C deps). Always check if the user is on Apple Silicon and proactively suggest a remote builder.

**Remote builder setup** (requires an amd64 Linux machine with SSH access):

```bash
docker buildx create --name <builder-name> --driver docker-container --platform linux/amd64 "ssh://<host>"
docker buildx inspect <builder-name> --bootstrap
export DOCKER_REMOTE_BUILDER="<builder-name>"
```

All service Pulumi stacks read `DOCKER_REMOTE_BUILDER` and offload the build to the remote machine.

**Alternative: use pre-built GHCR images.** If the user doesn't have a remote builder, guide them through this:

```bash
# 1. Trigger a build with a custom tag
gh workflow run docker-services.yml --repo boundless-xyz/boundless --ref main -f custom_tag="my-dev-build"

# 2. Wait for it to finish
gh run list --workflow=docker-services.yml --repo boundless-xyz/boundless -L 1

# 3. Login to GHCR (required — registry is private)
echo $(gh auth token) | docker login ghcr.io -u $(gh api user --jq .login) --password-stdin

# 4. Deploy with the pre-built image
export GHCR_IMAGE_TAG="my-dev-build"  # or "latest" for latest main build
pulumi config set USE_GHCR true
pulumi up
```

IMPORTANT: The GHCR login step is required before `pulumi up` when using `USE_GHCR`. Without it, the image existence check will fail with 401. Always remind the user to run the `docker login ghcr.io` command.

## Teardown workflow

Per service:

```bash
cd infra/<service>
export DEV_NAME="<name>"
pulumi destroy
```

Full environment (destroy services first, then bootstrap):

```bash
cd infra/bootstrap
export DEV_NAME="<name>"
pulumi destroy
```

## Available services

| Service         | Directory               | Description                           |
| --------------- | ----------------------- | ------------------------------------- |
| bootstrap       | `infra/bootstrap`       | VPC, deployment role (one-time setup) |
| sample          | `infra/sample`          | Smoke test (S3 buckets)               |
| slasher         | `infra/slasher`         | Slash enforcement                     |
| indexer         | `infra/indexer`         | Market/rewards event indexer          |
| order-stream    | `infra/order-stream`    | Off-chain order submission            |
| order-generator | `infra/order-generator` | Test order generation                 |
| distributor     | `infra/distributor`     | Token distribution                    |

## Troubleshooting

### "Missing required configuration variable"

The service needs env vars. Read `index.ts` for `getEnvVar()` calls and export them.

### "StackReference not found"

Set `BASE_STACK=organization/bootstrap/services-dev` and make sure bootstrap is deployed.

### "already exists"

Resource name conflict — either another developer is using the same `DEV_NAME`, or a previous stack wasn't cleaned up. Check with `pulumi stack ls`.

### "Access Denied" / "not authorized"

Re-authenticate: `aws-vault exec boundless-dev -- aws sts get-caller-identity`

### "pulumi: command not found"

Install: `brew install pulumi` or `curl -fsSL https://get.pulumi.com | sh`

### Docker build fails with `Error 127` / OpenSSL errors / exit code 101

The user is building amd64 Docker images on Apple Silicon. QEMU emulation fails on native C deps. Guide them to set up a remote Docker builder (see "Docker builds on Apple Silicon" section above).

### "Enter your passphrase to unlock config/secrets"

Tell the user to set `export PULUMI_CONFIG_PASSPHRASE=""` (empty string) for dev stacks.

## IMPORTANT

- NEVER run `pulumi destroy` without the user explicitly asking for it.
- NEVER touch staging or production stacks. This skill is for dev only.
- NEVER commit `Pulumi.dev.yaml` files — they contain local config.
- Always confirm the `DEV_NAME` with the user before deploying.
