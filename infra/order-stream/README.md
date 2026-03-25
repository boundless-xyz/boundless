# Order Stream Infrastructure

## Telemetry Pipeline

```
Broker --> Order Stream --> Kinesis Data Streams --> Redshift Materialized Views
                              (3 streams)              (raw SUPER JSON)
                                                            |
                                                     Typed SQL Views
                                                   (broker_heartbeats,
                                                    request_evaluations,
                                                    request_completions)
```

Three separate Kinesis streams are provisioned (heartbeats, evaluations, completions), each mapped to a raw materialized view in Redshift that stores the full JSON payload as a `SUPER` column.

Typed SQL views are then created from the materialized views, that extract and cast individual fields.

Stream retention is 72 hours. Data materialized into Redshift is permanent.

## Pulumi Components

- `components/telemetry.ts` - Kinesis streams, Redshift Serverless namespace/workgroup, IAM roles, security groups
- `components/order-stream.ts` - ECS service with Kinesis permissions and env vars

## Redshift Schema

The telemetry schema has two layers:

**Layer 1 - Raw materialized views** (`heartbeats_raw`, `evaluations_raw`, `completions_raw`): Each stores the full JSON payload from Kinesis as a single Redshift `SUPER` column called `data`. These are stable and never need schema changes. Whatever fields the broker sends in its JSON are stored as-is.

**Layer 2 - Typed SQL views** (`broker_heartbeats`, `request_evaluations`, `request_completions`): These extract and cast individual fields from the raw `data` column using dot notation (e.g. `data.broker_address::VARCHAR(42)`). They also handle deduplication via `ROW_NUMBER()`.

### Schema Evolution

Adding or removing telemetry fields does not require changes to the raw materialized views. The process is:

1. **Add/remove fields in the broker's Rust telemetry structs.** The raw layer automatically stores whatever JSON arrives.
2. **Update the typed views** (via a new migration) to extract any new fields or drop removed ones.

If a typed view references a JSON field that doesn't exist in a given record (e.g. older records from before the field was added, or newer records after the field was removed), Redshift returns `NULL` for that column rather than erroring. This means typed view columns are always populated on a best-effort basis - present fields are cast to the declared type, missing fields appear as `NULL`.

This design keeps the raw data intact and decouples ingestion from the query schema. You can retroactively add columns to the typed views and they will immediately work across all historical data (returning `NULL` for records that predate the field).

## Redshift Migrations

Migrations are numbered SQL files in `redshift-migrations/` (e.g. `001_initial_telemetry_tables.sql`) containing `${VAR}` placeholders that are substituted by `envsubst` at apply time.

To apply migrations manually:

```bash
export REDSHIFT_ADMIN_PASSWORD="admin-password"
export REDSHIFT_READONLY_PASSWORD="your-readonly-password"

./redshift-migrations/apply.sh dev
```

In deployment pipelines, apply script is run automatically after each deployment.

To add a new migration, create the next numbered file (e.g. `004_add_new_field.sql`) and re-run `apply.sh`. Since Redshift does not allow `CREATE OR REPLACE VIEW` when the column list changes, migrations that add or remove columns must `DROP VIEW` and recreate it (and re-grant permissions to the `readonly` user).

## Querying Redshift

Connect using any Postgres client with the read-only credentials:

```bash
psql "host=<endpoint> port=5439 dbname=telemetry user=readonly password=<password> sslmode=require"
```

```sql
SELECT * FROM telemetry.broker_heartbeats ORDER BY timestamp DESC LIMIT 10;
SELECT * FROM telemetry.request_evaluations ORDER BY evaluated_at DESC LIMIT 10;
SELECT * FROM telemetry.request_completions ORDER BY completed_at DESC LIMIT 10;
```
