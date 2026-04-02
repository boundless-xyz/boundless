---
name: logs-query
description: Query AWS CloudWatch logs for Boundless services (provers, slasher, distributor, order stream, order generator, indexer) on prod/staging environments. Use when the user asks to look at service logs, debug service behavior from log output, search logs for a request ID, or investigate errors using CloudWatch. Do NOT use for debugging local code changes, reviewing PRs, or investigating issues in the codebase itself.
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

### Known prover log groups

These are stable and won't change:

| Log Group                                       | Environment | Network      | Description                    |
| ----------------------------------------------- | ----------- | ------------ | ------------------------------ |
| `/boundless/bento/prover-84532-staging-nightly` | staging     | Base Sepolia | Staging nightly prover         |
| `/boundless/bento/base-mainnet-prover-nightly`  | prod        | Base         | Prod nightly prover            |
| `/boundless/bento/base-mainnet-prover-release`  | prod        | Base         | Prod release prover (backstop) |

We only have prover log groups for provers we operate. External provers do not have queryable logs.

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

## Tips

- Keep time windows as narrow as possible (minutes or hours, not days)
- Start with a request ID filter, then broaden if needed
- Log messages are typically structured (JSON or key=value), so `jq` is useful for parsing
- If the output is very large, add `| head -50` or pipe to a file
- Use the `--limit` flag to cap results per API call (default 10000)
- When unsure which log group to query, discover them first with `describe-log-groups`
- Some services span multiple log groups -- check all matching groups when investigating
