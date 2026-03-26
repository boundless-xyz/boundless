---
name: telemetry-query
description: Query Boundless broker telemetry tables on Redshift. Use when the user asks about broker health, request evaluations, request completions, proving times, skip rates, telemetry data, or wants to run SQL against the telemetry database.
---

# Telemetry Query

Query the Boundless telemetry database (Amazon Redshift Serverless, Postgres-compatible) for broker operational data.

## Prerequisites

Before running any queries, check if `REDSHIFT_URL` is already exported in the shell. If not, the user needs to provide two things:

1. The Redshift **endpoint** (hostname or full connection string)
2. The **readonly password**

They can either:

- Export them as env vars: `export REDSHIFT_ENDPOINT="..."` and `export REDSHIFT_PASSWORD="..."`
- Provide them directly in the chat

These credentials are available in the **Boundless runbook**. Tell the user to check there if they don't have them.

The user may provide a full `postgres://...` URL or just a hostname. Normalize to a valid connection string:

- If the URL is missing `/telemetry` at the end of the path, append it
- If the URL is missing `?sslmode=require`, append it
- If only a hostname is given, construct: `postgres://readonly:<password>@<hostname>:5439/telemetry?sslmode=require`

Export the result as `REDSHIFT_URL` before running queries.

## Connecting

Use `psql` to run queries:

```bash
psql "$REDSHIFT_URL" -c "<SQL>"
```

Or for multi-line queries, use a heredoc:

```bash
psql "$REDSHIFT_URL" <<'SQL'
SELECT ...
SQL
```

## Available Tables

All tables live in the `telemetry` schema. There are three views:

| View                            | Description                                                                           |
| ------------------------------- | ------------------------------------------------------------------------------------- |
| `telemetry.broker_heartbeats`   | Hourly broker health snapshots (config, queue sizes, version, uptime)                 |
| `telemetry.request_evaluations` | Per-order evaluation decisions (accepted/skipped, skip reasons, cycle counts, timing) |
| `telemetry.request_completions` | Terminal order outcomes (success/failure, proving durations, cycle counts)            |

## Schema Reference

**IMPORTANT**: Before writing any queries, you MUST read `crates/boundless-market/src/telemetry.rs` to get the exact column names and enum values. The view columns map 1:1 to the Rust struct fields (snake_case). Do NOT guess column names.

The key mappings are:

- `BrokerHeartbeat` -> `telemetry.broker_heartbeats`
- `RequestEvaluated` -> `telemetry.request_evaluations`
- `RequestCompleted` -> `telemetry.request_completions`

The Redshift views also include a `received_at` TIMESTAMPTZ column (when Kinesis ingested the record) that is not part of the Rust structs.

## Query Guidelines

- Always use `telemetry.` schema prefix
- Views are deduplicated via `ROW_NUMBER()` on `order_id` -- no need to dedupe in your queries
- Timestamps are `TIMESTAMPTZ` -- use standard Postgres date functions
- `broker_address` is a `VARCHAR(42)` Ethereum address (e.g. `0xabc...`)
- `outcome` values are defined by enums in the Rust source -- read them there, do not assume
- Use `LIMIT` on exploratory queries -- tables can be large
- For time ranges, filter on `evaluated_at`, `completed_at`, or `timestamp` columns

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
