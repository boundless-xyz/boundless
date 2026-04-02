---
name: query
description: Cross-reference Boundless indexer API data, broker telemetry, and service logs to investigate production and staging operational issues. Use when the user wants to understand why slashings happened on prod/staging, diagnose prover or service failures in deployed environments, correlate market events with broker behavior, investigate fulfillment rate drops, look at prover/service logs, or perform any analysis that requires combining on-chain indexer data with off-chain broker telemetry and CloudWatch logs. Also use when the user asks to "investigate", "diagnose", or "find root cause" for prover, service, or market issues on live networks. Do NOT use for debugging local code changes, reviewing PRs, or investigating issues in the codebase itself.
---

# Query

Combine on-chain indexer data, off-chain broker telemetry, and CloudWatch service logs to investigate operational issues and find insights.

## Setup

Set up the data sources needed for the investigation. Not all sources are needed for every query -- use the ones relevant to the task.

1. **Read `network_secrets.toml`** from the repo root. If it exists, it contains credentials for all environments (indexer API keys, telemetry DB URLs/passwords, AWS creds). Also read `network_address_labels.json` (same directory) for labelling addresses -- it is plain JSON (`{"0xaddr": "label", ...}`). If `network_secrets.toml` is not present, recommend the user create it -- instructions and credentials are in the **Boundless runbook**. If `network_address_labels.json` is not present, recommend the user create it -- the canonical address mapping is in the **Boundless runbook**.

2. **Read and follow the indexer-query skill** at `.claude/skills/indexer-query/SKILL.md` to set up indexer access (`MARKET_INDEXER_URL`, `ZKC_INDEXER_URL`, optional `INDEXER_API_KEY`, and the `indexer_get` helper function).

3. **Read and follow the telemetry-query skill** at `.claude/skills/telemetry-query/SKILL.md` to set up Redshift access (`REDSHIFT_URL`). Before writing any telemetry SQL, read `crates/boundless-market/src/telemetry.rs` for exact column names and enum values.

4. **Read and follow the logs-query skill** at `.claude/skills/logs-query/SKILL.md` to set up CloudWatch log access (AWS credentials, log group discovery). Use when the investigation benefits from raw service logs -- especially for our own operated provers, or for other services like indexer, order stream, slasher, etc.

5. Ask the user which **network** they want to investigate (determines which market indexer + ZKC indexer to use).

## Data Source Overview

| Source                   | What it knows                                                                                                                                      | Join fields                                                  |
| ------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| **Market Indexer**       | On-chain request lifecycle: submitted, locked, fulfilled, slashed, expired. Pricing, collateral, tx hashes, timestamps.                            | `request_id`, `request_digest`, prover/requestor addresses   |
| **Telemetry (Redshift)** | Broker-side operational data: evaluation decisions, skip reasons, proving durations, error codes, queue depths, estimated vs actual proving times. | `request_id`, `request_digest`, `broker_address`, `order_id` |
| **CloudWatch Logs**      | Raw service logs for provers we operate (and other infra services). Detailed error messages, stack traces, runtime behavior.                       | `request_id`, `request_digest`, timestamps                   |

The key join between indexer and telemetry is **`request_id`** and **`request_digest`**. The indexer's `lock_prover_address` or `fulfill_prover_address` corresponds to telemetry's `broker_address`. Logs can be correlated by searching for the same `request_id` or `request_digest` within the relevant time window.

Telemetry is **opt-in** -- not all brokers send telemetry. If a prover address has no telemetry data, note this to the user. CloudWatch logs are only available for services we operate.

## Investigation Workflow

All investigations follow the same pattern:

1. **Identify targets** -- Use the indexer to find the relevant requests/addresses/time periods.
2. **Correlate with telemetry** -- Use the request IDs, digests, or broker address + time window to look up telemetry data.
3. **Dig into logs** (if applicable) -- Search CloudWatch logs for the same request IDs/digests within the time window to get raw error messages, stack traces, or detailed runtime context.
4. **Analyze and synthesize** -- Combine findings from all sources into a coherent narrative.

Rate limit all sources:

- Indexer: `sleep 1` between requests, `sleep 2` between pagination pages.
- Redshift: No rate limit, but use `LIMIT` on exploratory queries.
- CloudWatch: `sleep 1` between paginated log queries.

## Investigations

For detailed step-by-step investigation procedures, read [references/investigations.md](references/investigations.md).

Available investigations:

| Investigation              | When to use                                                                                                     |
| -------------------------- | --------------------------------------------------------------------------------------------------------------- |
| **Slashing Reasons**       | Prover was slashed on one or more requests. Find out why -- proving failures, estimation errors, overload, etc. |
| **Fulfillment Rate Drops** | Market or prover fulfillment rate declined. Correlate with telemetry error patterns.                            |
| **Prover Performance**     | Deep dive into a prover's operational health -- proving speeds, error rates, capacity utilization.              |
| **Request Lifecycle**      | Trace a specific request from submission through evaluation, proving, and completion across both data sources.  |

If the user's question doesn't map to a specific investigation, use the general pattern: identify targets in the indexer, then correlate with telemetry.
