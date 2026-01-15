# Market Indexer

## Development on AWS

```
export ORDER_STREAM_URL="" 
export ORDER_STREAM_API_KEY="" 
export TX_FETCH_STRATEGY="tx-by-hash" 
export LOGS_ETH_RPC_URL="https://base-mainnet.g.alchemy.com/v2/" 
export ETH_RPC_URL="https://x.base-mainnet.quiknode.pro//"
export RDS_PASSWORD=""
export DEV_NAME=""
export CHAIN_ID=""
# Required for proxy-authenticated forwarded-IP rate limiting (CloudFront WAF)
export PROXY_SECRET=""
# Optional
export DOCKER_REMOTE_BUILDER=

pulumi login --local
pulumi stack init dev
pulumi config set CHAIN_ID x

# If you are deploying the market indexer
pulumi config set BOUNDLESS_MARKET_ADDR x 

# If you are deploying the rewards indexer
pulumi config set VEZKC_ADDRESS x
pulumi config set ZKC_ADDRESS x
pulumi config set POVW_ACCOUNTING_ADDRESS x
pulumi up
```

# Market Indexer Backfill

## Overview

In staging/prod, the market indexer backfill runs as an on-demand ECS Fargate task, triggered via EventBridge daily.

It is also possible to trigger a backfill manually. This involves executing a lambda function, which spawns the ECS task.

## Prerequisites

- AWS CLI v2
- jq (JSON parsing)
- Node.js & npm (for building Lambda)
- IAM permissions: `lambda:InvokeFunction`

## Manual backfill

Assume you have deployed the infra stack already.

### Get the Lambda Function Name

From Pulumi:

```bash
pulumi stack output backfillLambdaName
```

From AWS CLI:

```bash
aws lambda list-functions --query 'Functions[?contains(FunctionName, `backfill-trigger`)].FunctionName'
```

### Using the Script

```bash
./scripts/trigger-backfill.sh \
  --lambda-name <from above> \
  --mode statuses_and_aggregates \
  --start-block 35060420 \
  --end-block 39383104
```

### (Backup, prefer script) Direct Lambda Invocation

```bash
aws lambda invoke \
  --function-name dev-backfill-trigger \
  --payload '{"mode":"aggregates","startBlock":1000000,"endBlock":2000000}' \
  --cli-binary-format raw-in-base64-out \
  response.json && cat response.json
```

## Parameters

**Required:**

- `mode`: "statuses_and_aggregates" or "aggregates"
- `startBlock`: Starting block number

**Optional:**

- `endBlock`: Ending block number (default: latest indexed)
- `txFetchStrategy`: "block-receipts" or "tx-by-hash" (default: "tx-by-hash")

## Development

The Lambda source is in `backfill-trigger-lambda/src/index.ts`. After making changes:

```bash
cd backfill-trigger-lambda
npm run build
cd ..
pulumi up
```
