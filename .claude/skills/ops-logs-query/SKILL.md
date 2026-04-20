---
name: ops-logs-query
description: Internal — for Boundless team members only. Query AWS CloudWatch logs for Boundless services (provers, slasher, distributor, order stream, order generator, indexer, signal) on prod/staging environments. Use when the user asks to look at service logs, debug service behavior from log output, search logs for a request ID, or investigate errors using CloudWatch. Do NOT use for debugging local code changes, reviewing PRs, or investigating issues in the codebase itself.
---

# Logs Query

Query AWS CloudWatch Logs for Boundless services on prod/staging.

## Prerequisites

1. **Read `network_secrets.toml`** from the repo root. Extract the AWS credentials for the target environment from `[aws.prod]` or `[aws.staging]` (`access_key_id`, `secret_access_key`). If the file is not present, recommend the user create it -- instructions and credentials are in the **Boundless runbook**.

2. Export credentials before running any queries:

```bash
export AWS_ACCESS_KEY_ID="..."
export AWS_SECRET_ACCESS_KEY="..."
export AWS_DEFAULT_REGION="us-west-2"
```

## Finding Log Groups

### Prover log groups

Prover log groups follow the pattern `/boundless/bento/<hostname>`. The hostnames are defined in the Pulumi config files:

- **Staging**: `infra/cw-monitoring/Pulumi.staging.yaml`
- **Prod**: `infra/cw-monitoring/Pulumi.production.yaml`

Read the relevant Pulumi config to find current hostnames. As of now:

| Log Group                                       | Environment | Chain ID               | Label (approx, in `network_address_labels`) | Description                                                                      |
| ----------------------------------------------- | ----------- | ---------------------- | ------------------------------------------- | -------------------------------------------------------------------------------- |
| `/boundless/bento/prover-84532-staging-nightly` | staging     | 84532 (Base Sepolia)   |                                             | Staging nightly prover                                                           |
| `/boundless/bento/prover-8453-prod-release`     | prod        | 8453 (Base)            | `BPLatitudeRelease`                         | Prod release prover. Legacy name: `/boundless/bento/base-mainnet-prover-release` |
| `/boundless/bento/prover-8453-prod-nightly`     | prod        | 8453 (Base)            | `BPLatitudeNightly`                         | Prod nightly prover. Legacy name: `/boundless/bento/base-mainnet-prover-nightly` |
| `/boundless/bento/prover-84532-prod-nightly`    | prod        | 84532 (Base Sepolia)   |                                             | Prod Base Sepolia nightly prover                                                 |
| `/boundless/bento/prover-11155111-prod-nightly` | prod        | 11155111 (Eth Sepolia) |                                             | Prod Eth Sepolia nightly prover                                                  |
| `/boundless/bento/prover-01`                    | prod        | 8453 (Base)            | `BPProver01DC`                              | Prod datacenter prover 01                                                        |
| `/boundless/bento/prover-02`                    | prod        | 8453 (Base)            | `BPProver02DC`                              | Prod datacenter prover 02                                                        |

The label column lists the `network_address_labels.json` label that the log group is believed to correspond to. Names may not match exactly -- always confirm against `network_address_labels.json` and Pulumi config before relying on the mapping.

If these are out of date, check the Pulumi config files for the current list.

We only have prover log groups for provers we operate. External provers do not have queryable logs.

Provers we operate are labeled with a **`BP` prefix** in `network_address_labels.json` (e.g. `BP1`, `BP2`, `BPNightlyAWS`). When investigating any issue, always highlight what our BP provers are doing -- did they skip, fail, drop, or fulfill? This should be called out explicitly even when the investigation is not specifically about our provers.

### Discovering log groups for other services

Other services (slasher, distributor, order stream, order generator, indexer backend, indexer API, etc.) have log groups that follow a naming convention but may change. **Discover them dynamically** rather than hardcoding.

Log group naming convention: `l-<staging|prod>-<chain_id>-<service-name>-<chain_id>-<resource>`

Example: `l-staging-167000-indexer-api-167000-lambda`

Known chain IDs:

- `84532` = Base Sepolia
- `8453` = Base Mainnet
- `167000` = Taiko Mainnet
- `11155111` = Eth Sepolia
- `1` = Eth Mainnet

To find log groups for a specific service, search by prefix. Use the environment (staging/prod) and optionally the chain ID and service name:

```bash
aws logs describe-log-groups \
  --log-group-name-prefix "l-staging-84532" \
  --query 'logGroups[].logGroupName' --output table
```

To find log groups for a specific service across all chains in an environment:

```bash
aws logs describe-log-groups \
  --query 'logGroups[?contains(logGroupName, `staging`) && contains(logGroupName, `indexer`)].logGroupName' \
  --output table
```

To list all log groups in an environment:

```bash
aws logs describe-log-groups \
  --log-group-name-prefix "l-staging" \
  --query 'logGroups[].logGroupName' --output table
```

Also check bento prover log groups:

```bash
aws logs describe-log-groups \
  --log-group-name-prefix "/boundless/bento" \
  --query 'logGroups[].logGroupName' --output table
```

Some services have multiple log groups for different components (e.g. an indexer may have separate groups for the backend worker and the API lambda). When investigating an issue, check all matching log groups.

### Service name patterns

Common service name fragments to search for:

| Service         | Search fragments                                           |
| --------------- | ---------------------------------------------------------- |
| Indexer API     | `indexer-api`                                              |
| Indexer backend | `indexer`, `market-indexer`, `rewards-indexer`             |
| Order stream    | `order-stream`                                             |
| Order generator | `order-generator`, `og`                                    |
| Slasher         | `slasher`                                                  |
| Distributor     | `distributor`                                              |
| Signal          | `prod-8453-signal` (no `l-` prefix)                        |
| Prover (bento)  | `/boundless/bento/prover` or `/boundless/bento/*-prover-*` |

## Querying Logs

**Always filter by time range.** Log groups are high-volume and queries without time bounds will be slow or hit limits.

Use `aws logs filter-log-events` for searching. Key parameters:

- `--log-group-name`: required
- `--start-time` / `--end-time`: Unix milliseconds (required -- always set these)
- `--filter-pattern`: CloudWatch filter syntax for searching log content
- `--output json`: pipe through jq for readability

### Computing timestamps

Convert human-readable times to Unix milliseconds:

```bash
START=$(date -j -u -f "%Y-%m-%dT%H:%M:%SZ" "2026-03-30T00:00:00Z" +%s 2>/dev/null || date -d "2026-03-30T00:00:00Z" +%s)
START_MS=$((START * 1000))

END=$(date -j -u -f "%Y-%m-%dT%H:%M:%SZ" "2026-03-31T00:00:00Z" +%s 2>/dev/null || date -d "2026-03-31T00:00:00Z" +%s)
END_MS=$((END * 1000))
```

For relative times:

```bash
NOW_MS=$(date +%s)000
ONE_HOUR_AGO_MS=$(( ($(date +%s) - 3600) * 1000 ))
SIX_HOURS_AGO_MS=$(( ($(date +%s) - 21600) * 1000 ))
ONE_DAY_AGO_MS=$(( ($(date +%s) - 86400) * 1000 ))
```

### Searching by request ID

The most common query pattern. Request IDs appear in log messages as hex values (e.g. `0x2a`):

```bash
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --start-time "$ONE_HOUR_AGO_MS" \
  --end-time "$NOW_MS" \
  --filter-pattern '"0xREQUEST_ID"' \
  --output json | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
```

### Searching by request digest

```bash
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --start-time "$ONE_HOUR_AGO_MS" \
  --end-time "$NOW_MS" \
  --filter-pattern '"0xDIGEST"' \
  --output json | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
```

### Searching for errors

```bash
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --start-time "$ONE_HOUR_AGO_MS" \
  --end-time "$NOW_MS" \
  --filter-pattern '"ERROR"' \
  --output json | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
```

### Searching across multiple log groups

When a service has multiple log groups, query each one:

```bash
for LG in "l-staging-84532-indexer-api-84532-lambda" "l-staging-84532-market-indexer-84532-task"; do
  echo "=== $LG ==="
  aws logs filter-log-events \
    --log-group-name "$LG" \
    --start-time "$ONE_HOUR_AGO_MS" \
    --end-time "$NOW_MS" \
    --filter-pattern '"ERROR"' \
    --output json | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
  sleep 1
done
```

## Pagination

`filter-log-events` returns a `nextToken` when there are more results:

```bash
TOKEN=""
while true; do
  if [ -n "$TOKEN" ]; then
    RESP=$(aws logs filter-log-events \
      --log-group-name "$LOG_GROUP" \
      --start-time "$START_MS" \
      --end-time "$END_MS" \
      --filter-pattern '"0xREQUEST_ID"' \
      --next-token "$TOKEN" \
      --output json)
  else
    RESP=$(aws logs filter-log-events \
      --log-group-name "$LOG_GROUP" \
      --start-time "$START_MS" \
      --end-time "$END_MS" \
      --filter-pattern '"0xREQUEST_ID"' \
      --output json)
  fi

  echo "$RESP" | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
  TOKEN=$(echo "$RESP" | jq -r '.nextToken // empty')
  [ -z "$TOKEN" ] && break
  sleep 1
done
```

## CloudWatch Filter Pattern Syntax

- `"exact phrase"` -- matches logs containing the exact phrase (quotes required)
- `?term1 ?term2` -- OR: matches logs containing either term
- `"term1" "term2"` -- AND: matches logs containing both terms
- `"ERROR" "request_id"` -- combine filters

## Checking for Recent Deployments

When investigating fulfillment rate drops, prover downtime, or success rate alarms for provers we operate, **always check for recent deployments first**. Nightly deployments restart the bento Docker Compose stack and can cause extended outages if the new image is broken.

Deployment events appear in the bento prover log groups (e.g. `/boundless/bento/prover-11155111-prod-nightly`). Look for these patterns:

```bash
# Find recent deployments in a time window
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --start-time "$START_MS" \
  --end-time "$END_MS" \
  --filter-pattern '?"Stopping Docker Compose" ?"Starting Docker Compose"' \
  --output json | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
```

A deployment cycle looks like:

1. `"Stopping Docker Compose services"` — old containers torn down
2. `"Image ghcr.io/boundless-xyz/boundless/broker:<tag> Pulling"` — new image pulled (the tag contains the git commit, e.g. `nightly-3b8a71f`)
3. `"Container bento-broker-1 Created"` / `"Starting"` — new containers come up
4. Optionally: `"dependency failed to start: container ... is unhealthy"` — a container failed its healthcheck, cascading to broker failure

Deployments are significant events -- they restart the broker (causing a brief gap in telemetry and fulfillments even when healthy) and deploy new code that could introduce bugs or behavior changes. Always note when a deployment occurred relative to the issue being investigated.

If the broker stopped fulfilling shortly after a deployment, check for:

- **Healthcheck failures**: `?"unhealthy" ?"failed to start" ?"Error dependency"` — a dependency container (often `rest_api`) failed, preventing the broker from starting
- **Container crashes**: `?"exit" ?"Exited" ?"Restarting"` — the broker or a dependency crashed after startup
- **Image tag**: compare the deployed image tag (git commit hash) against the git log to identify what changed

```bash
# Check for container failures after a deployment
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --start-time "$START_MS" \
  --end-time "$END_MS" \
  --filter-pattern '?"unhealthy" ?"failed to start" ?"Exited" ?"Error dependency"' \
  --output json | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
```

## Secondary Fulfillment in Logs

When a prover locks an order but fails to fulfill it before the lock expires, the order becomes available for **secondary fulfillment** by any other prover, who earns the slash collateral as reward. In broker logs, secondary fulfillment attempts appear as `FulfillAfterLockExpire` entries. When investigating expired or slashed requests, search our BP prover logs for the request ID to see if they evaluated the secondary fulfillment opportunity:

```bash
# Search for secondary fulfillment activity on a specific request
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --start-time "$START_MS" \
  --end-time "$END_MS" \
  --filter-pattern '"0xREQUEST_ID" "FulfillAfterLockExpire"' \
  --output json | jq '.events[] | {timestamp: (.timestamp / 1000 | todate), message: .message}'
```

If the request ID doesn't appear at all, the prover never saw the secondary opportunity. If it appears with skip or error messages, note the reason -- common issues include the order being unprofitable at the slash collateral price, insufficient remaining deadline, or the prover being at capacity. Always check whether our BP provers attempted secondary fulfillment on orders that expired after being locked.

## Tips

- Keep time windows as narrow as possible (minutes or hours, not days)
- Start with a request ID filter, then broaden if needed
- Log messages are typically structured (JSON or key=value), so `jq` is useful for parsing
- If the output is very large, add `| head -50` or pipe to a file
- Use the `--limit` flag to cap results per API call (default 10000)
- When unsure which log group to query, discover them first with `describe-log-groups`
- Some services span multiple log groups -- check all matching groups when investigating
