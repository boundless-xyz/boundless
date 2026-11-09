# Stop request producers

Deploy this branch through the existing order-generator and indexer pipelines. Complete the indexer production update before approving the generator production update, so low-traffic alarms are muted first. Do not create missing stacks.

`order-generator-base:STOPPED: true` scales generator services to zero, disables random-requestor schedules, blocks load-test Lambda invocation with reserved concurrency zero, and mutes generator alarm actions. Resources, logs, secrets and pipelines remain managed.

The protocol sunset now disables all indexer alarm actions, including customer, market and service alarms. Metrics and alarm state continue updating.

To resume, set the generator flag to false and deploy. Restoring alerts also requires reverting the sunset-wide alarm disablement and the 19 producer/volume overrides in `infra/indexer/alarmConfig.ts`. Existing desired counts still apply, including zero counts on previously stopped networks.

Already running standalone tasks and in-flight Lambda invocations are not cancelled by these settings. Verify they have exited before declaring generation stopped. The live inventory on October 28, 2026 found none in the scheduled/load-test clusters.

Signal has a separate stop branch based on `willpote/initial-signal`. Kailua launcher/watchdog changes have a separate branch based on `zeroecco/kailua_cleanup`; do not deploy the older Kailua project from main for this change.

Run `python3 infra/test-producer-stop.py og-service og-schedule load-test indexer` after installing each affected project's locked npm dependencies. The tests use Pulumi mocks to check generator stop/resume, the configured indexer alarm actions, and retention of resource identities and other inputs. They do not replace a live deployment preview.

## Stop order stream and slasher

`order-stream:DESIRED_COUNT: "0"` and `slasher:DESIRED_COUNT: "0"` scale both services to zero on every launch stack. The ALB, RDS databases, Redshift telemetry, logs and secrets remain.

The market indexer exits on any order-stream fetch failure, so `indexer:ORDER_STREAM_URL` is removed from every indexer stack and the indexer runs onchain-only. Deploy the indexer before order stream. Stop Kailua, which is configured with `BOUNDLESS_ORDER_STREAM_URL`, and any other offchain submitter first.

To resume, remove the `DESIRED_COUNT` overrides, deploy order stream, then restore the `ORDER_STREAM_URL` values from git history and deploy the indexer.
