# Infrastructure — Developer Guide

Deploy Boundless services to AWS for development and testing.

> **Tip:** If you use Claude Code, run `/infra-deploy` to get interactive help with setup, deployments, and troubleshooting.

## Quick start

```bash
# 0. Start an AWS session (stays active for the shell session, no need to prefix commands)
aws-vault exec boundless-dev -s -d 12h

# 1. Bootstrap (one-time)
cd infra/bootstrap
export DEV_NAME="<your-name>" PULUMI_CONFIG_PASSPHRASE=""
pulumi login --local && pulumi stack init services-dev && pulumi install
pulumi up

# 2. Deploy a service (e.g. slasher)
cd infra/slasher
export DEV_NAME="<your-name>" BASE_STACK="organization/bootstrap/services-dev" PULUMI_CONFIG_PASSPHRASE=""
export CHAIN_ID="..." ETH_RPC_URL="..." PRIVATE_KEY="..." BOUNDLESS_MARKET_ADDR="..." RDS_PASSWORD="..."
# Apple Silicon? Use a remote builder (see below)
# export DOCKER_REMOTE_BUILDER="<builder-name>"
pulumi login --local && pulumi stack init dev && pulumi install
pulumi up

# 3. Tear down
pulumi destroy
```

## Prerequisites

Install these tools before starting:

- **Node.js** (v18+) — `brew install node` or [nodejs.org](https://nodejs.org)
- **Pulumi CLI** — `brew install pulumi` or `curl -fsSL https://get.pulumi.com | sh`
- **AWS CLI** — `brew install awscli`
- **aws-vault** — `brew install --cask aws-vault` (manages AWS credentials securely)

You also need access to the **Boundless Dev AWS account**. Ask your team lead to add you as a developer in AWS IAM Identity Center (SSO).

## Architecture

```console
infra/
├── bootstrap/       # VPC, deployment role, ECR config (one-time setup per developer)
├── sample/          # Smoke test project (S3 bucket)
├── distributor/     # Distributor service
├── indexer/         # Market/rewards indexer
├── order-generator/ # Order generator
├── order-stream/    # Off-chain order submission
├── slasher/         # Slash enforcement
├── builder/         # Builder-base Docker image
├── cw-monitoring/   # CloudWatch alarms/dashboards
├── pipelines/       # CI/CD CodePipeline definitions
├── requestor-lists/ # Requestor allowlists
└── util/            # Shared helpers (chain IDs, naming, GHCR image resolution)
```

Each service has its own Pulumi project. Development stacks use local Pulumi state and deploy to the dev AWS account. Staging/prod stacks use S3-backed state and KMS-encrypted secrets.

## First-time setup

### 1. Configure aws-vault

Add the dev account profile to `~/.aws/config`:

```ini
[profile boundless-dev]
region=us-west-2
sso_start_url="https://d-92678e206d.awsapps.com/start"
sso_region=us-west-2
sso_account_id = <dev-account-id>
sso_role_name=BoundlessDevelopmentAdmin
```

Test it works:

```bash
aws-vault exec boundless-dev -- aws sts get-caller-identity
```

### 2. Bootstrap your dev environment

This creates a personal VPC, deployment role, and ECR config. Only needed once.

```bash
cd infra/bootstrap

# Set your name — used as a prefix for all resources you create.
# Use lowercase, no spaces (e.g. "angelo", "jonas").
export DEV_NAME="<your-name>"

pulumi login --local
pulumi stack init services-dev
pulumi install
aws-vault exec boundless-dev -- pulumi up
```

### 3. Verify with the sample project

```bash
cd ../sample
export DEV_NAME="<your-name>"
export SAMPLE_SECRET="test123"

pulumi login --local
pulumi stack init dev
pulumi install
aws-vault exec boundless-dev -- pulumi up
```

If successful, you'll see S3 buckets created. Clean up when done:

```bash
aws-vault exec boundless-dev -- pulumi destroy
```

## Deploying a service

All services follow the same pattern. Example with the slasher:

```bash
cd infra/slasher
export DEV_NAME="<your-name>"
export BASE_STACK="organization/bootstrap/services-dev"
export PULUMI_CONFIG_PASSPHRASE=""  # avoids passphrase prompt for dev stacks

# Set any required env vars for the service (check the service's index.ts for getEnvVar calls)
export ETH_RPC_URL="https://..."
export PRIVATE_KEY="0x..."
# ...

pulumi login --local
pulumi stack init dev
pulumi install
aws-vault exec boundless-dev -- pulumi up
```

### Environment variables

In dev mode, services read secrets from environment variables (not Pulumi secrets). Check each service's `index.ts` for `getEnvVar()` calls to see what's required.

### Docker builds on Apple Silicon (M-series Macs)

Service Docker images target `linux/amd64`. Building them locally on Apple Silicon via QEMU emulation is **very slow and often fails** (especially when compiling native C dependencies like OpenSSL).

**Recommended: use a remote Docker builder.** If you have an amd64 Linux machine (e.g. a dev server), set it up as a remote builder:

```bash
# One-time setup — create the remote builder (requires SSH key access)
docker buildx create --name <builder-name> --driver docker-container --platform linux/amd64 "ssh://<host>"

# Verify it works
docker buildx inspect <builder-name> --bootstrap

# Use it when deploying — set this before `pulumi up`
export DOCKER_REMOTE_BUILDER="<builder-name>"
```

All service Pulumi stacks read `DOCKER_REMOTE_BUILDER` and will offload the build to the remote machine.

**Alternative: use pre-built GHCR images.** The `docker-services` CI workflow builds images on every push to `main`. You can trigger it manually and use the resulting image (see `USE_GHCR` config flag in staging/prod stacks).

### Tearing down

```bash
aws-vault exec boundless-dev -- pulumi destroy
```

To remove the entire dev environment including VPC:

```bash
# Destroy all services first, then bootstrap
cd infra/bootstrap
aws-vault exec boundless-dev -- pulumi destroy
```

## Creating a new service

```bash
cd infra
mkdir <service-name> && cd <service-name>

pulumi login --local
pulumi new aws-typescript --name <service-name> --stack dev --yes
```

Add `Pulumi.dev.yaml` to `.gitignore` (dev stacks contain local config, not committed).

Add a build script to `package.json` (used by CI):

```json
{
  "scripts": {
    "build": "tsc"
  }
}
```

Reference the bootstrap VPC in your `index.ts`:

```typescript
const baseStackName = isDev ? getEnvVar("BASE_STACK") : config.require('BASE_STACK');
const baseStack = new pulumi.StackReference(baseStackName);
const vpcId = baseStack.getOutput('VPC_ID');
const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');
```

## Deploying to staging and production

Requires **Ops Admin** permissions for Pulumi state bucket and KMS key access.

```bash
# Login to shared Pulumi backend
pulumi login "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"

# Create stacks with KMS-encrypted secrets
pulumi stack init --secrets-provider="awskms:///arn:aws:kms:us-west-2:968153779208:alias/pulumi-secrets-key" l-staging-<chain-id>
pulumi stack init --secrets-provider="awskms:///arn:aws:kms:us-west-2:968153779208:alias/pulumi-secrets-key" l-prod-<chain-id>

# Set secrets
pulumi config set --secret <NAME> <VALUE>
```

Staging deploys automatically on merge to `main` via CodePipeline. Production requires manual approval in the AWS CodePipeline console.

## Troubleshooting

### `Error: Missing required configuration variable`

Check which env vars the service needs — look for `getEnvVar()` calls in `index.ts`. In dev mode, all config comes from environment variables.

### `Error: pulumi:pulumi:StackReference ... not found`

Make sure you set `BASE_STACK` to your bootstrap stack name:

```bash
export BASE_STACK="organization/bootstrap/services-dev"
```

### `Error: could not create resource ... already exists`

Your `DEV_NAME` prefix may conflict with another developer. Use a unique name.

### `aws-vault: command not found`

Install aws-vault: `brew install --cask aws-vault`

### Docker build fails with `Error 127` or OpenSSL compilation errors

You're likely building on Apple Silicon without a remote builder. See [Docker builds on Apple Silicon](#docker-builds-on-apple-silicon-m-series-macs) above.

### `Enter your passphrase to unlock config/secrets`

Set `PULUMI_CONFIG_PASSPHRASE=""` (empty string) for dev stacks. Add it to your `~/.zshrc` to avoid the prompt permanently.

### Permission errors

Make sure you're authenticated: `aws-vault exec boundless-dev -- aws sts get-caller-identity`
