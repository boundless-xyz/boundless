# Indexer Infrastructure

This directory contains the Pulumi infrastructure-as-code for deploying the Boundless indexers to AWS.

## AWS Architecture

The indexer infrastructure runs on AWS with the following components:

```
                                      +------------------+
                                      |   RPC Providers  |
                                      |  (Alchemy, etc.) |
                                      +--------+---------+
                                               |
            +----------------------------------+----------------------------------+
            |                                                                     |
            v                                                                     v
+------------------------+                                             +------------------------+
|  ECS Fargate Service   |                                             |  ECS Fargate Task      |
|  (long-running)        |                                             |  (on-demand)           |
|                        |                                             |                        |
|  +------------------+  |                                             |  +------------------+  |
|  | Market Indexer   |  |                                             |  | Backfill Task    |  |
|  +------------------+  |                                             |  +------------------+  |
+----------+-------------+                                             +----------+-------------+
           |                                                                      ^
           |                                                                      |
           |    +------------------------------------------+                      |
           |    |           Aurora PostgreSQL              |                      |
           |    |                                          |                      |
           +--->|  +----------------+  +----------------+  |<---------------------+
                |  | Writer         |  | Reader         |  |
                |  | Instance       |  | Instance       |  |
                |  | (db.r6g.xlarge)|  | (db.r6g.xlarge)|  |
                |  +-------+--------+  +--------+-------+  |
                |          |                    |          |
                |     writes only          reads only      |
                +------------------------------------------+
                                                |
                                                v
+-----------------------------------------------------------------------------------+
|                                    AWS VPC                                        |
+-----------------------------------------------------------------------------------+

=== API Layer ===

+---------------------+      +----------------------+      +------------------+
|   Frontend Apps     |----->|  API Gateway +       |----->|  Indexer API     |
|                     |      |  CloudFront          |      |  (Lambda)        |
+---------------------+      +----------------------+      +--------+---------+
                                                                    |
                                                                    | reads from
                                                                    v
                                                           Aurora Reader Instance

=== Operations / Backfill ===

+-------------------------+      +----------------------+      +------------------+
|     EventBridge         |----->|  Backfill Trigger    |----->| Backfill Task    |
|  (scheduled triggers)   |      |  (Lambda)            |      | (ECS Fargate)    |
|                         |      +----------------------+      +------------------+
|  - Daily aggregates     |                                           |
|    backfill (2 AM UTC)  |                                           | writes to
|  - Daily chain data     |                                           v
|    backfill (6 PM UTC)  |                                   Aurora Writer Instance
+-------------------------+

=== Supporting Services ===

+---------------------------+     +------------------------------------------+
|          ECR              |     |              S3 Bucket                   |
|   (container registry)    |     |   (cache for logs/tx metadata)          |
+---------------------------+     +------------------------------------------+
```

### Data Flow

**Indexing**

The Market Indexer runs as a long-running ECS Fargate service. It continuously fetches blockchain events from RPC providers (e.g., Alchemy, QuikNode), processes them, and writes to the Aurora PostgreSQL writer instance. The indexer tracks its progress via the `last_block` table and can be configured to lag behind chain head to reduce reorg risk.

**API Reads**

The Indexer API Lambda serves read requests from frontend applications. It connects to the Aurora PostgreSQL reader instance to avoid impacting write performance on the writer. CloudFront sits in front of API Gateway to provide:

- Edge caching for frequently accessed data (reduces Lambda invocations and database load)
- Geographic distribution for lower latency
- Rate limiting via WAF to protect against abuse

**Backfills**

Backfills reprocess historical data to fix inconsistencies or rebuild computed tables. They run as on-demand ECS Fargate tasks triggered by Lambda functions.

**Scheduled Backfills**:

| Schedule          | Mode                      | Purpose                                                                                  |
| ----------------- | ------------------------- | ---------------------------------------------------------------------------------------- |
| Daily at 2 AM UTC | `statuses_and_aggregates` | Recomputes all request statuses and regenerates aggregation tables                       |
| Daily at 6 PM UTC | `chain_data`              | Re-fetches recent blockchain events (default: last 100k blocks) to catch any missed data |

**Backfill Modes**:

- **`chain_data`**: Re-fetches and processes raw blockchain events from RPC. Used to backfill missing events or re-index after schema changes that affect event storage. Processes in batches with configurable delay to avoid RPC rate limits.

- **`statuses_and_aggregates`**: Iterates through all request digests and recomputes their status from the stored event data, then regenerates all aggregation tables. Used when status computation logic changes or to fix data inconsistencies.

- **`aggregates`**: Regenerates only the aggregation tables (market, requestor, prover summaries) without touching request statuses. Used when aggregation logic changes or to rebuild summaries after manual data fixes.

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

or chain_data mode with lookback blocks:

```bash
./scripts/trigger-backfill.sh \
  --lambda-name <from above> \
  --mode chain_data \
  --lookback-blocks 1000
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

- `mode`: "statuses_and_aggregates", "aggregates", or "chain_data"
- `startBlock` OR `lookbackBlocks`: Starting block number OR number of blocks to look back from current

**Optional:**

- `endBlock`: Ending block number (default: latest indexed)
- `txFetchStrategy`: "block-receipts" or "tx-by-hash" (default: "tx-by-hash")

**Note:** You must provide either `startBlock` or `lookbackBlocks`, but not both. The `lookbackBlocks` parameter is useful when you want to backfill a fixed number of blocks from the current chain head.

## Development

The Lambda source is in `backfill-trigger-lambda/src/index.ts`. After making changes:

```bash
cd backfill-trigger-lambda
npm run build
cd ..
pulumi up
```
