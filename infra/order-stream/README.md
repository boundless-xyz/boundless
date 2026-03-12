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

## Redshift Migrations

Migrations are numbered SQL files in `redshift-migrations/` (e.g. `001_initial_telemetry_tables.sql`) containing `${VAR}` placeholders that are substituted by `envsubst` at apply time.

To apply migrations manually:

```bash
export REDSHIFT_ADMIN_PASSWORD="admin-password"
export REDSHIFT_READONLY_PASSWORD="your-readonly-password"

./redshift-migrations/apply.sh dev
```

In deployment pipelines, apply script is run automatically after each deployment.

To add a new migration, create the next numbered file (e.g. `002_add_new_field.sql`) and re-run `apply.sh`.

## Querying Redshift

Connect using any Postgres client with the read-only credentials:

```bash
psql "postgres://<user>>:<password>@<redshift url>:<redshift port>/telemetry?sslmode=require"
```

```sql
SELECT * FROM telemetry.broker_heartbeats ORDER BY timestamp DESC LIMIT 10;
SELECT * FROM telemetry.request_evaluations ORDER BY evaluated_at DESC LIMIT 10;
SELECT * FROM telemetry.request_completions ORDER BY completed_at DESC LIMIT 10;
```
