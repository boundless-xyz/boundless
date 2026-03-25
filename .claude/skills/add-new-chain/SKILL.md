---
name: add-new-chain
description: >-
  Deploy Boundless infrastructure to a new blockchain. Creates Pulumi stacks,
  config files, updates ChainId enum, and wires CI/CD pipelines for staging
  and prod environments. Use when adding a new chain, deploying to a new
  network, or setting up new chain infrastructure.
---

# Add New Chain to Boundless Infrastructure

Interactive workflow for deploying Boundless services (order-stream, distributor,
slasher, indexer, order-generator) to a new blockchain network.

## SAFETY: Never perform destructive operations

NEVER delete, remove, or destroy Pulumi stacks, resources, or config files.
NEVER run `pulumi destroy`, `pulumi stack rm`, or remove existing `Pulumi.*.yaml`
files. This skill is strictly additive -- it only creates new stacks and config.

NEVER perform destructive AWS actions. Do not delete AWS resources, S3 objects,
KMS keys, IAM roles, security groups, RDS instances, ECS services, or any other
AWS infrastructure. Do not run `aws ... delete-*`, `aws ... remove-*`, or
`aws ... terminate-*` commands.

## Step 1: Gather Chain Information

Ask the user for the following information in batches of 2-3 questions:

### Batch 1: Chain basics

- **Chain ID** (numeric, e.g. `167000` for Taiko)
- **Chain name** (human-readable, e.g. `Taiko Mainnet`)
- **Do staging and prod use different chain IDs?** (e.g. mainnet for prod, testnet for staging, or same chain ID with different contracts)

### Batch 2: Contract addresses

Staging and prod typically have different contract deployments. Ask for **two
separate sets** of contract addresses -- one for staging and one for prod. Some
addresses (e.g. `COLLATERAL_TOKEN_ADDRESS`, `SET_VERIFIER_ADDR`) may be shared
across environments; confirm with the user rather than assuming.

For each environment (staging and prod), ask for:

- `BOUNDLESS_ADDRESS` (BoundlessMarket contract -- also used as `BOUNDLESS_MARKET_ADDR` in some services)
- `COLLATERAL_TOKEN_ADDRESS`
- `SET_VERIFIER_ADDR`
- `CHAINALYSIS_ORACLE_ADDRESS` (use zero address if chain has no Chainalysis oracle)

If the user does not have an address yet, use
`0x0000000000000000000000000000000000000000` as a placeholder.

Note: `DISTRIBUTOR_ADDRESS` is the same across all staging envs and all prod
envs respectively. It gets copied automatically from the source stack via
`copy_config.sh` and does not need to be asked for or overridden.

### Batch 3: Deployment details

- **Deployment block** for each environment (block number where contracts were
  deployed -- used as `START_BLOCK` for indexer and slasher). Staging and prod
  may have different deployment blocks if they use different contracts.
- **RPC URLs** for the new chain -- ask for a separate RPC URL per service, since
  each service may use a different provider. Services that need an RPC URL:
  - order-stream (`ETH_RPC_URL`)
  - distributor (`ETH_RPC_URL`)
  - slasher (`ETH_RPC_URL`)
  - indexer (`ETH_RPC_URL` and optionally `LOGS_ETH_RPC_URL`)
  - order-generator (`ETH_RPC_URL`)
    The user may provide a single URL for all, or different URLs per service.
- **ALB domain** for order-stream (e.g. `taiko-mainnet.boundless.network`), or skip if not setting up a domain yet

Store all gathered values as variables for use in subsequent steps.

## Step 2: Verify AWS Access

The dev account role only has read-only access to the Pulumi state bucket.
You need credentials for a role with write access: **Ops Admin**, **Staging Admin**,
or **Production Admin**.

Ask the user to provide AWS credentials. They need to export these env vars in
the terminal:

```bash
export AWS_ACCESS_KEY_ID="..."
export AWS_SECRET_ACCESS_KEY="..."
export AWS_SESSION_TOKEN="..."
export AWS_DEFAULT_REGION="us-west-2"
```

After the user confirms they have exported credentials, verify by running:

```bash
aws sts get-caller-identity
```

Confirm the output shows a valid identity. If it fails, stop and ask the user
to fix credentials before proceeding.

Then verify Pulumi backend access:

```bash
pulumi login "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
```

If either command fails, do not proceed.

## Step 3: Update ChainId Enum

Edit `infra/util/index.ts`:

1. Add the new chain to the `ChainId` enum (use UPPER_SNAKE_CASE):

```typescript
export enum ChainId {
  ETH_MAINNET = "1",
  ETH_SEPOLIA = "11155111",
  BASE = "8453",
  BASE_SEPOLIA = "84532",
  {NEW_CHAIN} = "{CHAIN_ID}",
}
```

2. Add entry in `getChainName()` (before the `throw`):

```typescript
if (chainId === ChainId.{NEW_CHAIN}) {
  return "{Chain Name}";
}
```

3. Add entry in `getChainId()` (before the `throw`):

```typescript
if (chainId === "{CHAIN_ID}") {
  return ChainId.{NEW_CHAIN};
}
```

## Step 4: Update Pipeline Definitions

Two files need updating. The others inherit from `LaunchDefaultPipeline` and
get the new chain automatically.

### 4a: LaunchDefaultPipeline.ts

File: `infra/pipelines/pipelines/base/LaunchDefaultPipeline.ts`

In `createPipeline()`, after the existing CodeBuild projects, add two new ones:

```typescript
const stagingDeployment{ChainName} = new aws.codebuild.Project(
    `l-${this.config.appName}-staging-{CHAIN_ID}-build`,
    this.codeBuildProjectArgs(this.config.appName, "l-staging-{CHAIN_ID}", role, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
    { dependsOn: [role] }
);

const prodDeployment{ChainName} = new aws.codebuild.Project(
    `l-${this.config.appName}-prod-{CHAIN_ID}-build`,
    this.codeBuildProjectArgs(this.config.appName, "l-prod-{CHAIN_ID}", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
    { dependsOn: [role] }
);
```

Add pipeline stage actions:

- In `DeployStaging.actions` array, add action with `runOrder: 1`:

```typescript
{
    name: "DeployStaging{ChainName}",
    category: "Build",
    owner: "AWS",
    provider: "CodeBuild",
    version: "1",
    runOrder: 1,
    configuration: {
        ProjectName: stagingDeployment{ChainName}.name
    },
    outputArtifacts: ["staging_output_{chain_snake}"],
    inputArtifacts: ["source_output"],
}
```

- In `DeployProduction.actions` array, add action with `runOrder: 2`:

```typescript
{
    name: "DeployProduction{ChainName}",
    category: "Build",
    owner: "AWS",
    provider: "CodeBuild",
    version: "1",
    runOrder: 2,
    configuration: {
        ProjectName: prodDeployment{ChainName}.name
    },
    outputArtifacts: ["production_output_{chain_snake}"],
    inputArtifacts: ["source_output"],
}
```

### 4b: l-indexer.ts

File: `infra/pipelines/pipelines/l-indexer.ts`

This file overrides `createPipeline()` with its own stage definitions. Apply the
exact same pattern as 4a -- add staging + prod CodeBuild projects and pipeline
stage actions.

### Files that do NOT need changes

- `l-distributor.ts`, `l-order-stream.ts`, `l-slasher.ts`, `l-order-generator.ts` use `LaunchDefaultPipeline` without overriding `createPipeline()`
- `l-requestor-lists.ts` uses environment-level stacks (`l-staging`/`l-prod`), not chain-specific

## Step 5: Create Pulumi Stacks and Config Files

### Constants (do not change these)

| Constant           | Value                                                                   |
| ------------------ | ----------------------------------------------------------------------- |
| Pulumi backend     | `s3://boundless-pulumi-state?region=us-west-2&awssdk=v2`                |
| Secrets provider   | `awskms:///arn:aws:kms:us-west-2:968153779208:alias/pulumi-secrets-key` |
| Copy config script | `infra/util/copy_config.sh`                                             |

### Services to deploy

| Service         | Directory               | Staging source stack | Prod source stack |
| --------------- | ----------------------- | -------------------- | ----------------- |
| order-stream    | `infra/order-stream`    | `l-staging-84532`    | `l-prod-8453`     |
| distributor     | `infra/distributor`     | `l-staging-84532`    | `l-prod-8453`     |
| slasher         | `infra/slasher`         | `l-staging-84532`    | `l-prod-8453`     |
| indexer         | `infra/indexer`         | `l-staging-84532`    | `l-prod-8453`     |
| order-generator | `infra/order-generator` | `l-staging-84532`    | `l-prod-8453`     |

### For each service, run these commands

If staging and prod use different chain IDs, substitute appropriately.

```bash
cd infra/{service}

# Staging
pulumi stack init l-staging-{CHAIN_ID} \
  --secrets-provider "awskms:///arn:aws:kms:us-west-2:968153779208:alias/pulumi-secrets-key"
../util/copy_config.sh l-staging-84532 l-staging-{CHAIN_ID}

# Prod
pulumi stack init l-prod-{CHAIN_ID} \
  --secrets-provider "awskms:///arn:aws:kms:us-west-2:968153779208:alias/pulumi-secrets-key"
../util/copy_config.sh l-prod-8453 l-prod-{CHAIN_ID}

cd ../..
```

After all stacks are created, ensure the new config files are writable:

```bash
chmod u+w infra/*/Pulumi.l-*-{CHAIN_ID}.yaml
```

### Override chain-specific config values

After copying, override these for BOTH staging and prod stacks. Use `--secret`
for sensitive values. Replace `{env}` with `staging` or `prod`.

Use the **staging contract addresses** for staging stacks and the **prod contract
addresses** for prod stacks. The placeholders below use `{BOUNDLESS_ADDRESS}`,
`{COLLATERAL_TOKEN_ADDRESS}`, `{SET_VERIFIER_ADDR}`, `{CHAINALYSIS_ORACLE_ADDRESS}`,
and `{START_BLOCK}` -- substitute the correct environment-specific values.

#### order-stream

```bash
pulumi config set order-stream:CHAIN_ID "{CHAIN_ID}" --stack l-{env}-{CHAIN_ID}
pulumi config set order-stream:BOUNDLESS_ADDRESS "{BOUNDLESS_ADDRESS}" --stack l-{env}-{CHAIN_ID}
pulumi config set order-stream:ETH_RPC_URL "{RPC_URL}" --secret --stack l-{env}-{CHAIN_ID}
pulumi config set order-stream:BASE_STACK "organization/bootstrap/services-{env}" --stack l-{env}-{CHAIN_ID}
```

If ALB_DOMAIN was provided:

```bash
pulumi config set order-stream:ALB_DOMAIN "{ALB_DOMAIN}" --stack l-{env}-{CHAIN_ID}
```

#### distributor

`DISTRIBUTOR_ADDRESS` is copied from the source stack and does not need overriding.

```bash
pulumi config set distributor:CHAIN_ID "{CHAIN_ID}" --stack l-{env}-{CHAIN_ID}
pulumi config set distributor:BOUNDLESS_MARKET_ADDR "{BOUNDLESS_ADDRESS}" --stack l-{env}-{CHAIN_ID}
pulumi config set distributor:ETH_RPC_URL "{RPC_URL}" --secret --stack l-{env}-{CHAIN_ID}
pulumi config set distributor:CHAINALYSIS_ORACLE_ADDRESS "{CHAINALYSIS_ORACLE_ADDRESS}" --stack l-{env}-{CHAIN_ID}
pulumi config set distributor:BASE_STACK "organization/bootstrap/services-{env}" --stack l-{env}-{CHAIN_ID}
```

#### slasher

```bash
pulumi config set slasher:CHAIN_ID "{CHAIN_ID}" --stack l-{env}-{CHAIN_ID}
pulumi config set slasher:BOUNDLESS_MARKET_ADDR "{BOUNDLESS_ADDRESS}" --stack l-{env}-{CHAIN_ID}
pulumi config set slasher:ETH_RPC_URL "{RPC_URL}" --secret --stack l-{env}-{CHAIN_ID}
pulumi config set slasher:START_BLOCK "{START_BLOCK}" --stack l-{env}-{CHAIN_ID}
pulumi config set slasher:BASE_STACK "organization/bootstrap/services-{env}" --stack l-{env}-{CHAIN_ID}
```

#### indexer

```bash
pulumi config set indexer:CHAIN_ID "{CHAIN_ID}" --stack l-{env}-{CHAIN_ID}
pulumi config set indexer:BOUNDLESS_ADDRESS "{BOUNDLESS_ADDRESS}" --stack l-{env}-{CHAIN_ID}
pulumi config set indexer:ETH_RPC_URL "{RPC_URL}" --secret --stack l-{env}-{CHAIN_ID}
pulumi config set indexer:START_BLOCK "{START_BLOCK}" --stack l-{env}-{CHAIN_ID}
pulumi config set indexer:BASE_STACK "organization/bootstrap/services-{env}" --stack l-{env}-{CHAIN_ID}
```

Set `ORDER_STREAM_URL` to blank -- the correct URL is not known until the
order-stream service is deployed for the new chain:

```bash
pulumi config set indexer:ORDER_STREAM_URL "" --stack l-{env}-{CHAIN_ID}
```

#### order-generator

```bash
pulumi config set order-generator-base:CHAIN_ID "{CHAIN_ID}" --stack l-{env}-{CHAIN_ID}
pulumi config set order-generator-base:BOUNDLESS_MARKET_ADDR "{BOUNDLESS_ADDRESS}" --stack l-{env}-{CHAIN_ID}
pulumi config set order-generator-base:COLLATERAL_TOKEN_ADDRESS "{COLLATERAL_TOKEN_ADDRESS}" --stack l-{env}-{CHAIN_ID}
pulumi config set order-generator-base:ETH_RPC_URL "{RPC_URL}" --secret --stack l-{env}-{CHAIN_ID}
pulumi config set order-generator-base:SET_VERIFIER_ADDR "{SET_VERIFIER_ADDR}" --stack l-{env}-{CHAIN_ID}
pulumi config set order-generator-base:BASE_STACK "organization/bootstrap/services-{env}" --stack l-{env}-{CHAIN_ID}
```

Set `ORDER_STREAM_URL` to blank -- same as indexer, the URL is not known yet:

```bash
pulumi config set order-generator-base:ORDER_STREAM_URL "" --stack l-{env}-{CHAIN_ID}
```

**Remind the user**: After the order-stream service is deployed for the new
chain, come back and update `ORDER_STREAM_URL` in both the indexer and
order-generator stacks (staging and prod) with the actual URL.

## Step 6: Verify

1. Confirm all config files exist (10 files expected):

```bash
find infra -name "Pulumi.l-*-{CHAIN_ID}.yaml" | sort
```

2. Verify CHAIN_ID is set in each:

```bash
for f in $(find infra -name "Pulumi.l-*-{CHAIN_ID}.yaml"); do
  echo "$f: $(grep CHAIN_ID $f)"
done
```

3. Verify the ChainId enum compiles:

```bash
cd infra/pipelines && npm run build
```

## Naming Conventions Reference

| Element           | Pattern                             | Example                               |
| ----------------- | ----------------------------------- | ------------------------------------- |
| Stack name        | `l-{env}-{chainId}`                 | `l-staging-167000`                    |
| Config file       | `Pulumi.l-{env}-{chainId}.yaml`     | `Pulumi.l-staging-167000.yaml`        |
| CodeBuild project | `l-{appName}-{env}-{chainId}-build` | `l-order-stream-staging-167000-build` |
| Pipeline action   | `DeployStaging{ChainName}`          | `DeployStagingTaiko`                  |
| Output artifact   | `{env}_output_{chain_snake}`        | `staging_output_taiko`                |
