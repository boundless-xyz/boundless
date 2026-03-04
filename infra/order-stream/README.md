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

Three separate Kinesis streams are provisioned (heartbeats, evaluations, completions), each mapped to a raw materialized view in Redshift that stores the full JSON payload as a `SUPER` column. Typed SQL views on top extract and cast individual fields, making schema evolution straightforward -- only the views need updating.

Stream retention is 72 hours. Data materialized into Redshift is permanent.

## Pulumi Components

- `components/telemetry.ts` - Kinesis streams, Redshift Serverless namespace/workgroup, IAM roles, security groups
- `components/order-stream.ts` - ECS service with Kinesis permissions and env vars

Telemetry is toggled via `ENABLE_TELEMETRY` in the Pulumi config. When enabled, the `REDSHIFT_ADMIN_PASSWORD` secret must also be set.

## Redshift Migrations

Migrations are numbered SQL files in `redshift-migrations/` (e.g. `001_initial_telemetry_tables.sql`) containing `${VAR}` placeholders that are substituted by `envsubst` at apply time.

To apply migrations:

```bash
export REDSHIFT_ADMIN_PASSWORD="admin-password"
export REDSHIFT_READONLY_PASSWORD="your-readonly-password"

./redshift-migrations/apply.sh dev
```

The script fetches the Redshift endpoint, Kinesis stream names, and IAM role ARN from Pulumi stack outputs automatically. Only the two passwords need to be provided manually. Replace `dev` with the target stack name.

To add a new migration, create the next numbered file (e.g. `002_add_new_field.sql`) and re-run `apply.sh`.

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
