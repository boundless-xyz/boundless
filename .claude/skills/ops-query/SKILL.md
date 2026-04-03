---
name: ops-query
description: Internal — for Boundless team members only. Cross-reference Boundless indexer API data, broker telemetry, and service logs to investigate production and staging operational issues. Use when the user wants to understand why slashings happened on prod/staging, diagnose prover or service failures in deployed environments, correlate market events with broker behavior, investigate fulfillment rate drops, look at prover/service logs, or perform any analysis that requires combining on-chain indexer data with off-chain broker telemetry and CloudWatch logs. Also use when the user asks to "investigate", "diagnose", or "find root cause" for prover, service, or market issues on live networks. Do NOT use for debugging local code changes, reviewing PRs, or investigating issues in the codebase itself.
---

# Query

Combine on-chain indexer data, off-chain broker telemetry, and CloudWatch service logs to investigate operational issues and find insights.

## Setup

Set up the data sources needed for the investigation. Not all sources are needed for every query -- use the ones relevant to the task.

1. **Read `network_secrets.toml`** from the repo root. If it exists, it contains credentials for all environments (indexer API keys, telemetry DB URLs/passwords, AWS creds). Also read `network_address_labels.json` (same directory) for labelling addresses -- it is plain JSON (`{"0xaddr": "label", ...}`). If `network_secrets.toml` is not present, recommend the user create it -- instructions and credentials are in the **Boundless runbook**. If `network_address_labels.json` is not present, recommend the user create it -- the canonical address mapping is in the **Boundless runbook**.

2. **Read and follow the ops-indexer-query skill** at `.claude/skills/ops-indexer-query/SKILL.md` to set up indexer access (`MARKET_INDEXER_URL`, `ZKC_INDEXER_URL`, optional `INDEXER_API_KEY`, and the `indexer_get` helper function).

3. **Read and follow the ops-telemetry-query skill** at `.claude/skills/ops-telemetry-query/SKILL.md` to set up Redshift access (`REDSHIFT_URL`). Before writing any telemetry SQL, read `crates/boundless-market/src/telemetry.rs` for exact column names and enum values.

4. **Read and follow the ops-logs-query skill** at `.claude/skills/ops-logs-query/SKILL.md` to set up CloudWatch log access (AWS credentials, log group discovery). Use when the investigation benefits from raw service logs -- especially for our own operated provers, or for other services like indexer, order stream, slasher, etc.

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
2. **Correlate with telemetry** -- Use the request IDs, digests, or broker address + time window to look up telemetry data. Telemetry provides a pre-processed view with structured skip reasons, error codes, proving durations, and estimation accuracy -- this should answer most questions about why orders were skipped, failed, or slashed without needing raw logs.
3. **Dig into logs** (last resort) -- Only go to CloudWatch logs if the indexer and telemetry data are insufficient. Logs are useful when you need raw error messages, stack traces, or runtime details that telemetry doesn't capture (e.g. infrastructure-level failures, panics, or non-prover service issues). One particularly useful check: **look for recent deployments** in bento prover logs. Nightly deployments restart Docker Compose and can explain gaps in telemetry, sudden behavior changes, or outages. New code deployed can also introduce bugs. See the "Checking for Recent Deployments" section in the ops-logs-query skill.
4. **Analyze and synthesize** -- Combine findings from all sources into a coherent narrative.

Rate limit all sources:

- Indexer: `sleep 1` between requests, `sleep 2` between pagination pages.
- Redshift: No rate limit, but use `LIMIT` on exploratory queries.
- CloudWatch: `sleep 1` between paginated log queries.

## Pre-Built Investigations

**Before starting any work, check if the user's question matches a pre-built investigation.** These are tested playbooks with the right queries, presentation format, and step-by-step instructions. Using them produces consistent, comprehensive results. Each lives in its own file under `references/`:

| Investigation              | File                                                                         | When to use                                                                                      |
| -------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| **Market Summary**         | [references/market-summary.md](references/market-summary.md)                 | "How's the market?" / "give me a summary" / overview of health, prover activity, failures, skips |
| **Slashing Reasons**       | [references/slashing-reasons.md](references/slashing-reasons.md)             | Prover was slashed -- find out why                                                               |
| **Fulfillment Rate Drops** | [references/fulfillment-rate-drops.md](references/fulfillment-rate-drops.md) | Market or prover fulfillment rate declined, success rate alarms                                  |
| **Prover Performance**     | [references/prover-performance.md](references/prover-performance.md)         | Deep dive into a specific prover's operational health                                            |
| **Request Lifecycle**      | [references/request-lifecycle.md](references/request-lifecycle.md)           | Trace a specific request end-to-end across all data sources                                      |

If the user's question clearly maps to one of these, read the corresponding file and follow it step by step. If it doesn't fit any pre-built investigation, fall back to the general Investigation Workflow above and build a custom investigation.

## Presenting Results

### Addresses

Always show the **full address** when displaying broker/prover addresses. Do not truncate to `0x8305...04b5`. If a label exists in `network_address_labels.json`, show both: `0x83052f16a84e6f2cec4bf3beda45c40c800904b5 (BP1)`.

### Prover Summary Tables

When showing prover activity, pivot telemetry outcomes into columns so each prover is one row. Include fulfilled, failures, and skips as separate columns. By default summary tables should cover the **top 5 provers by volume** plus **all provers we operate** (from address labels).

### Failure and Skip Breakdowns

After the summary table, include two separate breakdown sections:

**Failure breakdown**: For each prover, show a per-prover table of `outcome`, `error_code`, summarized `error_reason`, and count. Group by error pattern, not by individual request.

**Skip breakdown**: Same structure — per-prover table of `skip_code`, example reason, and count, sorted by count descending.

**Drop breakdown**: Same structure — per-prover table of `commitment_skip_code`, reason, and count, sorted by count descending.

### Telemetry Terminology

- **Locked**: Order was priced and the broker decided to try locking it on-chain.
- **Skipped**: Order was rejected during pricing in the OrderPicker (e.g. unprofitable, wrong image, over capacity). It never reached the OrderMonitor.
- **Committed**: Order was successfully committed to the proving pipeline (lock tx succeeded or immediate commitment for FulfillAfterLockExpire).
- **Dropped**: Order reached the OrderMonitor but was NOT committed to the proving pipeline. Reasons include: lock tx failed, order was fulfilled/expired/locked by another prover before we could act, insufficient deadline remaining, or insufficient balance. Check `commitment_skip_code` for specifics.
- **Cancelled**: Completion outcome meaning the broker finished proving but another prover fulfilled the order first. This is a race loss (wasted proving work), NOT an error. Do not count `Cancelled` as a failure in summary tables — show it as a separate column.
