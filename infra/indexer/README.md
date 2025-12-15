# Market Indexer

## Development Local

```
```

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
The market indexer backfill runs as an on-demand ECS Fargate task, triggered via Lambda function.

In production we periodically run a backfill automatically. This README lets you run a backfill manually.

## Prerequisites
- AWS CLI v2
- jq (JSON parsing)
- Node.js & npm (for building Lambda)
- IAM permissions: `lambda:InvokeFunction`

## Setup

Assume you have deployed the infra stack already.

## Get the Lambda Function Name

From Pulumi:
```bash
pulumi stack output backfillLambdaName
```

From AWS CLI:
```bash
aws lambda list-functions --query 'Functions[?contains(FunctionName, `backfill-trigger`)].FunctionName'
```

## Usage

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

## Monitoring

View task logs:
```bash
aws logs tail /aws/ecs/<environment>-backfill --follow
```

Check task status:
```bash
aws ecs describe-tasks --cluster <cluster> --tasks <task-arn>
```

## Development

The Lambda source is in `backfill-trigger-lambda/src/index.ts`. After making changes:
```bash
cd backfill-trigger-lambda
npm run build
cd ..
pulumi up
```

## Architecture

### Components
1. **Docker Image**: `market-indexer-backfill` binary built from Rust source
2. **ECS Task Definition**: Fargate task with all network config prebaked
3. **Lambda Function**: TypeScript function that triggers ECS tasks
4. **IAM Roles**: Execution role for ECS, task role for S3/DB access, Lambda role for ECS triggering

### Infrastructure Flow
```
User → trigger-backfill.sh → Lambda Function → ECS RunTask → Fargate Task
```

All infrastructure configuration (cluster, subnets, security groups, RPC URLs, contract addresses, S3 buckets) is stored in Lambda environment variables, so users only need to specify backfill-specific parameters (mode, block range).

## Troubleshooting

### Lambda function not found
Ensure you've deployed with Pulumi and the Lambda was created successfully:
```bash
pulumi stack output backfillLambdaName
aws lambda get-function --function-name <lambda-name>
```

### Task fails to start
Check Lambda logs for errors:
```bash
aws logs tail /aws/lambda/<lambda-name> --follow
```

### Task starts but fails
Check the backfill task logs:
```bash
aws logs tail /aws/ecs/<environment>-backfill --follow
```

### Permission errors
Ensure your IAM user/role has `lambda:InvokeFunction` permission for the Lambda function.

