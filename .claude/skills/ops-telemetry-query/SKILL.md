---
name: ops-telemetry-query
description: Internal — for Boundless team members only. Query Boundless broker telemetry tables on Redshift for prod/staging operational data. Use when the user asks about broker health, request evaluations, request completions, proving times, skip rates, telemetry data, or wants to run SQL against the telemetry database on live networks. Also covers historical telemetry through 2026-04-24 stored as Parquet archives in S3 (queried via DuckDB). Do NOT use for debugging local code changes, reviewing PRs, or investigating issues in the codebase itself.
---

# Telemetry Query

Query Boundless broker telemetry across two backends:

- **On/after 2026-04-25** — Amazon Redshift (provisioned, Postgres-compatible) via `psql`.
- **Through 2026-04-24** — S3 Parquet archives (UNLOADed from the old Redshift Serverless at cutover) via DuckDB.

The UNLOAD per env happened at various times during 2026-04-24; treating all of 2026-04-24 UTC as "archive" avoids any risk of missing data that landed in the archive after midnight but before the UNLOAD moment. Parquet schemas are identical to the Redshift typed views, so column names, types, and enum values are the same on both sides. See [Routing by Time Window](#routing-by-time-window) for how to pick a backend.

## Prerequisites

Before running any queries, check if `REDSHIFT_URL` is already exported in the shell.

If not, try to read `network_secrets.toml` from the repo root. If it exists, extract the telemetry credentials for the selected environment from `[environments.<env>.telemetry]`:

- `db_url` -- the Redshift hostname:port/database?sslmode path
- `readonly_password` -- the readonly user password

Construct the connection string: `postgres://readonly:<password>@<db_url>`

Also read `network_address_labels.json` (same directory) for translating addresses to human-readable labels in output.

If `network_secrets.toml` is not present, recommend the user create it -- instructions and credentials are in the **Boundless runbook**. Alternatively, they can provide credentials directly:

1. The Redshift **endpoint** (hostname or full connection string)
2. The **readonly password**

They can either:

- Export them as env vars: `export REDSHIFT_ENDPOINT="..."` and `export REDSHIFT_PASSWORD="..."`
- Provide them directly in the chat

If `network_address_labels.json` is not present, recommend the user create it -- the canonical address mapping is in the **Boundless runbook**. The file is plain JSON (`{"0xaddr": "label", ...}`).

The user may provide a full `postgres://...` URL or just a hostname. Normalize to a valid connection string:

- If the URL is missing `/telemetry` at the end of the path, append it
- If the URL is missing `?sslmode=require`, append it
- If only a hostname is given, construct: `postgres://readonly:<password>@<hostname>:5439/telemetry?sslmode=require`

Export the result as `REDSHIFT_URL` before running queries.

## Routing by Time Window

Telemetry was cut over from Redshift Serverless to Redshift provisioned during **2026-04-24 UTC**. For routing purposes, the boundary is **`2026-04-25T00:00:00Z`** — all of 2026-04-24 is treated as archive territory. This avoids a subtle gap: the per-env UNLOAD ran at various times on 2026-04-24 (e.g. `15:54 UTC` for prod_base), so a midnight cutover would miss archive rows between `2026-04-24 00:00` and each env's UNLOAD moment.

Pick a backend based on the user's time filter (`evaluated_at`, `completed_at`, `timestamp`):

| Window                          | Backend                                                                                                                                                                                                                                                                           |
| ------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `end < 2026-04-25T00:00:00Z`    | Archive only — DuckDB + S3 Parquet (see [Archive Backend](#archive-backend-duckdb--s3-parquet))                                                                                                                                                                                   |
| `start >= 2026-04-25T00:00:00Z` | Live only — existing `psql` + Redshift path                                                                                                                                                                                                                                       |
| Crosses `2026-04-25T00:00:00Z`  | **Two separate queries**: run the archive query for everything through `2026-04-24 23:59:59Z` and the live query for everything from `2026-04-25 00:00:00Z` onwards. Present both result sets to the user and note that the window crossed the cutover. Do not try to auto-UNION. |
| No time window given            | Default to live only and call that out in the response, so the user can widen the range if they wanted history.                                                                                                                                                                   |

## Archive Backend (DuckDB + S3 Parquet)

### Bucket mapping

All buckets live in `us-west-2`. Layout inside every bucket: `s3://<bucket>/<stack>/<view>/NNNN_part_00.parquet`, where `<view>` ∈ `broker_heartbeats | request_evaluations | request_completions`. Each bucket contains one stack prefix — use the glob `s3://<bucket>/*/<view>/*.parquet`.

| env (skill name)       | chain id | bucket                                       |
| ---------------------- | -------- | -------------------------------------------- |
| `prod_taiko`           | 167000   | `telemetry-snapshot-prod-167000-04-24-26`    |
| `prod_base`            | 8453     | `telemetry-snapshot-prod-8453-04-24-26`      |
| `prod_base_sepolia`    | 84532    | `telemetry-snapshot-prod-84532-04-24-26`     |
| `staging_taiko`        | 167000   | `telemetry-snapshot-staging-167000-04-24-26` |
| `staging_base_sepolia` | 84532    | `telemetry-snapshot-staging-84532-04-24-26`  |

### Schema

Parquet column names and types match the Redshift typed views exactly (Redshift did the UNLOAD; schemas were preserved). Enum values (outcomes, skip codes, fulfillment types) are defined in `crates/boundless-market/src/telemetry.rs` — the same source of truth as the live path. `received_at` is present.

### Credentials

Read AWS creds from `network_secrets.toml` at the repo root:

- `[aws.prod]` for any `prod_*` env
- `[aws.staging]` for any `staging_*` env

Both have `s3:GetObject` on their respective buckets. Region is `us-west-2`.

### Running a query

Mirror the psql heredoc pattern. Install `httpfs` once per `duckdb` invocation, set the creds, then `read_parquet` the glob. Heredoc keeps zsh from mangling `!=`.

```bash
AWS_ACCESS_KEY_ID=<from network_secrets.toml> \
AWS_SECRET_ACCESS_KEY=<from network_secrets.toml> \
duckdb <<SQL
INSTALL httpfs; LOAD httpfs;
SET s3_region='us-west-2';
SET s3_access_key_id='${AWS_ACCESS_KEY_ID}';
SET s3_secret_access_key='${AWS_SECRET_ACCESS_KEY}';

SELECT skip_code, COUNT(*) AS count
FROM read_parquet('s3://telemetry-snapshot-staging-84532-04-24-26/*/request_evaluations/*.parquet')
WHERE outcome = 'Skipped'
  AND evaluated_at BETWEEN TIMESTAMP '2026-04-18 00:00:00+00'
                       AND TIMESTAMP '2026-04-25 00:00:00+00'
GROUP BY skip_code
ORDER BY count DESC;
SQL
```

### DuckDB vs Redshift SQL differences

When rewriting a Redshift query for the archive path, watch for these:

- Use `NOW()` instead of `GETDATE()`.
- `MEDIAN()` and `PERCENTILE_CONT()` have no single-aggregate restriction — multiple percentiles in one SELECT work fine.
- `DISTINCT ON` and `LATERAL` joins are supported.
- Timestamp literal form: `TIMESTAMP '2026-04-25 00:00:00+00'` (note the timezone).

### Fail-loud rules

- If the user asks for a pre-cutover window on an env not in the bucket table → stop and say the archive doesn't exist for that env.
- If DuckDB returns `HTTP 403` / AccessDenied → stop and report the bucket name and which `[aws.*]` profile was used; do not silently retry.
- If `network_secrets.toml` is missing the needed `[aws.*]` block → stop and ask the user.

## Known Addresses

If `network_address_labels.json` exists at the repo root, use it to label addresses in results. The file is plain JSON (`{"0xaddr": "label", ...}`) so the user can paste directly from the canonical mapping. When displaying addresses from query output, check if the address (case-insensitive) has a known label and show it alongside, e.g. `0xbdA9...5542 (BP1)`.

Provers we operate are labeled with a **`BP` prefix** (e.g. `BP1`, `BP2`, `BPNightlyAWS`). When investigating any issue, always highlight what our BP provers are doing -- did they skip, fail, drop, or fulfill? This should be called out explicitly even when the investigation is not specifically about our provers.

## Connecting

Prefer heredocs for all queries (avoids shell escaping issues):

```bash
psql "$REDSHIFT_URL" <<'SQL'
SELECT ...
SQL
```

For simple one-liners, `psql -c` works but beware of shell quoting (see below):

```bash
psql "$REDSHIFT_URL" -c "SELECT 1;"
```

## Shell Quoting

**CRITICAL**: zsh treats `!` as history expansion inside double-quoted strings. This means `!=` in `psql -c "..."` gets mangled to `\!=`, causing a Redshift syntax error.

**Always use `<>` instead of `!=` in SQL.** This is valid standard SQL and avoids the issue entirely:

```sql
-- Good
WHERE outcome <> 'Skipped'

-- Bad (breaks in zsh double quotes)
WHERE outcome != 'Skipped'
```

Alternatively, use a heredoc (`<<'SQL'`) which is immune to shell expansion.

## Available Tables

All tables live in the `telemetry` schema. There are three views:

| View                            | Description                                                                           |
| ------------------------------- | ------------------------------------------------------------------------------------- |
| `telemetry.broker_heartbeats`   | Hourly broker health snapshots (config, queue sizes, version, uptime)                 |
| `telemetry.request_evaluations` | Per-order evaluation decisions (accepted/skipped, skip reasons, cycle counts, timing) |
| `telemetry.request_completions` | Terminal order outcomes (success/failure, proving durations, cycle counts)            |

The same three view names exist as directories under each archive bucket (e.g. `s3://<bucket>/<stack>/broker_heartbeats/`), with matching schemas.

## Schema Reference

**IMPORTANT**: Before writing any queries, you MUST read `crates/boundless-market/src/telemetry.rs` to get the exact column names and enum values. The view columns map 1:1 to the Rust struct fields (snake_case). Do NOT guess column names.

The key mappings are:

- `BrokerHeartbeat` -> `telemetry.broker_heartbeats`
- `RequestEvaluated` -> `telemetry.request_evaluations`
- `RequestCompleted` -> `telemetry.request_completions`

The Redshift views also include a `received_at` TIMESTAMPTZ column (when Kinesis ingested the record) that is not part of the Rust structs.

## Data Coverage

Telemetry is **opt-in**. Brokers can operate without sending telemetry, so the data here only represents brokers that have opted in. Do not assume that the brokers visible in telemetry are the only ones active on the network.

## Secondary Fulfillment

When a prover locks an order but fails to fulfill it, the order becomes available for **secondary fulfillment** by any other prover, who earns the slash collateral as reward. In telemetry, secondary fulfillments are identified by `fulfillment_type = 'FulfillAfterLockExpire'` in both `request_evaluations` and `request_completions`. When investigating expired or slashed requests, filter by this fulfillment type to see which brokers evaluated the secondary opportunity and whether they skipped, committed, or failed. Always check whether our BP provers attempted secondary fulfillment.

## Query Guidelines

- Always use `telemetry.` schema prefix
- Views are deduplicated via `ROW_NUMBER()` on `order_id` -- no need to dedupe in your queries
- Timestamps are `TIMESTAMPTZ` -- use standard Postgres date functions
- `broker_address` is a `VARCHAR(42)` Ethereum address (e.g. `0xabc...`)
- `outcome` values are defined by enums in the Rust source -- read them there, do not assume
- Use `LIMIT` on exploratory queries -- tables can be large
- For time ranges, filter on `evaluated_at`, `completed_at`, or `timestamp` columns
- When the user's time window crosses 2026-04-25T00:00:00Z, follow [Routing by Time Window](#routing-by-time-window) — run the archive and live queries separately and present both result sets. Do not auto-UNION.

## Common Query Patterns

For ready-to-use analytical queries, read [example-queries.md](example-queries.md).

Quick examples:

```sql
-- Recent heartbeats
SELECT broker_address, version, uptime_secs, committed_orders_count, timestamp
FROM telemetry.broker_heartbeats ORDER BY timestamp DESC LIMIT 10;

-- Recent evaluations
SELECT broker_address, order_id, outcome, skip_code, total_cycles, evaluated_at
FROM telemetry.request_evaluations ORDER BY evaluated_at DESC LIMIT 10;

-- Recent completions
SELECT broker_address, order_id, outcome, proving_duration_secs, total_duration_secs, completed_at
FROM telemetry.request_completions ORDER BY completed_at DESC LIMIT 10;
```

## Redshift vs Postgres Differences

Redshift is mostly Postgres-compatible but has some differences:

- No `LATERAL` joins
- No `DISTINCT ON`
- `UNION ALL` with `ORDER BY` must be wrapped in a subquery (Redshift rejects bare `UNION ALL ... ORDER BY`)
- `MEDIAN()` / `PERCENTILE_CONT()` cannot be mixed with regular aggregates or with each other on different columns in the same SELECT -- use UNION ALL with one MEDIAN per subquery
- Use `GETDATE()` instead of `NOW()` (though `NOW()` works in some contexts)
- `SUPER` type for JSON -- use `JSON_PARSE()` and dot notation for access
- `APPROXIMATE COUNT(DISTINCT ...)` is available for large datasets via `APPROXIMATE COUNT(DISTINCT col)`
