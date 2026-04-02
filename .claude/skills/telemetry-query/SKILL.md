---
name: telemetry-query
description: Query Boundless broker telemetry tables on Redshift for prod/staging operational data. Use when the user asks about broker health, request evaluations, request completions, proving times, skip rates, telemetry data, or wants to run SQL against the telemetry database on live networks. Do NOT use for debugging local code changes, reviewing PRs, or investigating issues in the codebase itself.
---

# Telemetry Query

Query the Boundless telemetry database (Amazon Redshift Serverless, Postgres-compatible) for broker operational data.

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

## Known Addresses

If `network_address_labels.json` exists at the repo root, use it to label addresses in results. The file is plain JSON (`{"0xaddr": "label", ...}`) so the user can paste directly from the canonical mapping. When displaying addresses from query output, check if the address (case-insensitive) has a known label and show it alongside, e.g. `0xbdA9...5542 (BP1)`.

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

## Schema Reference

**IMPORTANT**: Before writing any queries, you MUST read `crates/boundless-market/src/telemetry.rs` to get the exact column names and enum values. The view columns map 1:1 to the Rust struct fields (snake_case). Do NOT guess column names.

The key mappings are:

- `BrokerHeartbeat` -> `telemetry.broker_heartbeats`
- `RequestEvaluated` -> `telemetry.request_evaluations`
- `RequestCompleted` -> `telemetry.request_completions`

The Redshift views also include a `received_at` TIMESTAMPTZ column (when Kinesis ingested the record) that is not part of the Rust structs.

## Data Coverage

Telemetry is **opt-in**. Brokers can operate without sending telemetry, so the data here only represents brokers that have opted in. Do not assume that the brokers visible in telemetry are the only ones active on the network.

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
