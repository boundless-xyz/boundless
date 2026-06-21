# Kailua Batch Runbook

This runbook covers the `zeroecco/INF-51` shipped code for the `kailua-batch`
service and the related production/staging tuning in the same branch.

## What Shipped

- New Pulumi app: `infra/kailua-batch`.
- New CodePipeline: `l-kailua-batch-pipeline`, currently sourcing
  `zeroecco/INF-51`.
- New scheduled ECS/Fargate workload for Kailua proof submission.
- New Lambda trigger that rate-limits ECS task launches by current running task
  count.
- CloudWatch logs, metric filters, and SEV2 alarms for task starts, task
  errors, skipped launches, and low-funds logs.
- Taiko indexer block delay increased from `10` to `30`.
- Taiko distributor/order-generator funding thresholds lowered and random
  requestor cadence reduced to `rate(48 hours)`.
- Indexer transaction fetches now retry RPC responses that return no transaction
  or a transaction with `block_number = null`; fatal errors print full chained
  context.
- Bento down alarms now wait 2 hours to match broker graceful shutdown behavior.

## Service Model

`kailua-batch` is a scheduled launcher, not a long-running ECS service.

EventBridge Scheduler invokes the trigger Lambda on staggered cron schedules.
The Lambda checks how many matching ECS tasks are already running. If the count
is below `MAX_RUNNING_TASKS`, it starts one Fargate task with schedule-specific
overrides:

- `START_BLOCK_OFFSET`
- `BLOCK_COUNT`

The Fargate container then runs:

```sh
FINALIZED=$(cast bn finalized -r "$L2_RPC")
START_BLOCK=$((FINALIZED - START_BLOCK_OFFSET))
cd /kailua
just prove "$START_BLOCK" "$BLOCK_COUNT" "$L1_RPC" "$L1_BEACON" "$L2_RPC" "$OP_NODE" "$DATA_REL" "$MODE" "$SEQ_WINDOW" "$LOG_VERBOSITY" 1 || [ $? -eq 111 ]
```

The service name is generated as:

- staging: `staging-kailua-batch-10`
- prod: `prod-kailua-batch-10`

## Key Config

The stacks are `staging` and `prod`.

Common:

- `CHAIN_ID`: `10`
- `KAILUA_IMAGE`: `ghcr.io/boundless-xyz/kailua:1343d65`
- `TASK_CPU`: `8`
- `TASK_MEMORY_MIB`: `8192`
- `SCHEDULE_WINDOW_MINUTES`: `20`
- `TASKS_PER_MINUTE`: `2`
- `SKIP_AWAIT_PROOF`: `true`
- `ASSIGN_PUBLIC_IP`: `false`

Staging:

- `BOUNDLESS_ORDER_STREAM_URL`: `https://eth-sepolia.boundless.network`
- `SCHEDULE_COUNT`: `1`
- `MAX_RUNNING_TASKS`: `4`
- PagerDuty disabled.

Prod:

- `BOUNDLESS_ORDER_STREAM_URL`: `https://base-mainnet.beboundless.xyz`
- `SCHEDULE_COUNT`: `20`
- `MAX_RUNNING_TASKS`: `40`
- PagerDuty disabled.

Required secrets:

- `WALLET_KEY`
- `L1_RPC`
- `L2_RPC`
- `OP_NODE`
- `L1_BEACON`
- `S3_URL`
- `S3_ACCESS_KEY` or `S3_UPLOAD_ACCESS_KEY`
- `S3_SECRET_KEY` or `S3_SECRET_ACCESS_KEY`
- `BOUNDLESS_RPC_URL`

## Deploy

Preferred deploy path is CodePipeline.

1. Open `l-kailua-batch-pipeline` in the ops AWS account.
2. Confirm the source revision is the intended branch/commit.
3. Let `DeployStaging` complete.
4. Verify staging health using the checks below.
5. Manually approve `DeployProduction`.
6. Verify prod health using the same checks.

Manual deploy, if the pipeline is unavailable:

```sh
cd infra/kailua-batch
pulumi login --non-interactive "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
pulumi install
npm run build
pulumi stack select staging
pulumi refresh --yes
pulumi up --yes
```

For prod, assume the production deployment role first and use
`pulumi stack select prod`.

## Health Checks

Use region `us-west-2`.

Check the trigger Lambda logs:

```sh
aws logs tail staging-kailua-batch-10-trigger --since 1h --region us-west-2
aws logs tail prod-kailua-batch-10-trigger --since 1h --region us-west-2
```

Healthy trigger signs:

- `KAILUA_TASK_STARTED`
- occasional `KAILUA_TASK_SKIPPED` only when running task count is at the
  configured cap.

Check the Fargate task logs:

```sh
aws logs tail staging-kailua-batch-10 --since 1h --region us-west-2
aws logs tail prod-kailua-batch-10 --since 1h --region us-west-2
```

Healthy task signs:

- `KAILUA_RUNNING finalized=... start_block=... offset=... block_count=...`
- proof request/order submission logs from Kailua.
- exit code `111` from `just prove` is treated as non-fatal by the container
  command.

Check running ECS tasks:

```sh
aws ecs list-tasks \
  --cluster prod-kailua-batch-10 \
  --family prod-kailua-batch-10 \
  --desired-status RUNNING \
  --region us-west-2
```

For staging, replace `prod` with `staging`.

## Alarms

All new service metrics are under:

```text
Boundless/Services/<stack>-kailua-batch-10
```

Important alarms:

- `<service>-not-running-SEV2`: no `KAILUA_RUNNING` log in 1 hour.
- `<service>-log-err-SEV2`: `ERROR` logs in 3 consecutive 20-minute periods.
- `<service>-log-bal-eth-SEV2`: low ETH balance log in 1 hour.
- `<service>-log-bal-stk-SEV2`: low stake/collateral balance log in 1 hour.

The trigger Lambda also emits a metric for `KAILUA_TASK_SKIPPED`. Skips are
expected if running tasks are at `MAX_RUNNING_TASKS`; investigate if skips are
continuous and the Fargate tasks are not completing.

## Triage

### Not-running Alarm

1. Check the trigger Lambda log group for recent invocations.
2. If there are no invocations, inspect EventBridge Scheduler rules named
   `<service>-m<minute>-t<taskIndex>`.
3. If invocations exist but no tasks start, look for missing environment
   variables, IAM failures, subnet/security-group errors, or `RunTask returned
   no task ARN`.
4. If tasks start but no `KAILUA_RUNNING` appears, inspect container startup
   logs and image pull status.

### Continuous Task Skips

1. List running ECS tasks for the cluster/family.
2. If tasks are genuinely still running, inspect task logs for long proofs,
   stuck RPC calls, or stalled uploads.
3. If no tasks are running but the Lambda still reports the cap, verify the
   Lambda `TASK_DEFINITION_FAMILY` matches the deployed ECS task family.
4. Increase `MAX_RUNNING_TASKS` only after confirming downstream RPC, S3/R2,
   and Boundless order stream capacity.

### Container Errors

Common failure areas:

- `cast bn finalized -r "$L2_RPC"` cannot reach the L2 RPC.
- `just prove` cannot reach `L1_RPC`, `L1_BEACON`, `L2_RPC`, or `OP_NODE`.
- R2/S3 upload fails due to `S3_URL`, `S3_ACCESS_KEY`, or `S3_SECRET_KEY`.
- Wallet lacks funds or collateral; look for `[B-BAL-ETH]` and `[B-BAL-STK]`.
- Boundless order stream submission fails.

### Low Funds

1. Identify which wallet emitted `[B-BAL-ETH]` or `[B-BAL-STK]`.
2. Top up native ETH for gas or stake/collateral as appropriate.
3. Confirm the alarm clears after the next successful run.

### Indexer RPC Errors

The indexer now retries only successful RPC responses that are incomplete:

- no transaction returned.
- transaction returned with `block_number = null`.

Transport/RPC errors still propagate to the outer retry/backoff path. When the
indexer exits, fatal messages should include full error chains via `{err:#}` and
`ServiceError` debug formatting. Use those chained errors to identify whether
the failure is database, RPC, event query, or transaction metadata related.

### Bento Down During Deploy

The bento down alarm now uses an 8 x 15-minute window, matching the broker's
2-hour graceful shutdown period. During deploys, the broker can stay
`deactivating` while waiting for committed orders to drain. In that window,
`bento_active=0` and chain-monitor `channel closed` noise can be expected.

Search broker logs for:

- `starting graceful shutdown`
- `in-progress orders to complete`
- `Cancelling critical tasks`

Escalate if bento is down beyond 2 hours or the logs do not show graceful
shutdown drain.

## Rollback

### Kailua Batch

Preferred rollback is to redeploy the last known good Pulumi state/source
revision through `l-kailua-batch-pipeline`.

To stop new task launches immediately without deleting resources, disable the
EventBridge Scheduler schedules for the affected service:

```sh
aws scheduler list-schedules --name-prefix prod-kailua-batch-10 --region us-west-2
aws scheduler update-schedule ... --state DISABLED
```

If the service must be removed, run `pulumi destroy` for the affected stack from
`infra/kailua-batch` after confirming no active tasks should continue.

### Config Tunings

Rollback the changed Pulumi config values and redeploy the affected component:

- indexer `BLOCK_DELAY`: `30` back to `10`.
- order-generator `AUTO_DEPOSIT`: `0.01` back to `0.015`.
- random requestor schedule: `rate(48 hours)` back to the prior cadence.
- distributor ETH thresholds/top-up amounts back to prior values.
- bento down alarm evaluation periods/datapoints from `8` back to `4` only if
  the broker graceful shutdown behavior or alerting policy also changes.

## Known Follow-ups

- The pipeline source branch is intentionally pinned to `zeroecco/INF-51` in
  this shipped branch. Change it back to `main` after the branch merges.
- PagerDuty is disabled for the new Kailua batch alarms in both staging and
  prod config.
- Prod currently points artifact storage config at staging-named R2 bucket/domain
  values; verify whether that is intentional before treating it as an incident.
