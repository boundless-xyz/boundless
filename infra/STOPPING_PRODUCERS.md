# Stop request producers

Deploy this branch through the existing order-generator and indexer pipelines. Complete the indexer production update before approving the generator production update, so low-traffic alarms are muted first. Do not create missing stacks.

`order-generator-base:STOPPED: true` scales generator services to zero, disables random-requestor schedules, blocks load-test Lambda invocation with reserved concurrency zero, and mutes generator alarm actions. Resources, logs, secrets and pipelines remain managed.

The 19 direct `actionsEnabled: false` entries in `infra/indexer/alarmConfig.ts` mute alarms for our requestor clients and aggregate submission/fulfillment volume. Customer alarms, expiry/slashing alarms for the market and indexer service alarms remain active. Metrics and alarm state continue updating.

To resume, set the generator flag to false and deploy. Verify generation before removing those 19 `actionsEnabled: false` entries and deploying the indexer. Existing desired counts still apply, including zero counts on previously stopped networks.

Already running standalone tasks and in-flight Lambda invocations are not cancelled by these settings. Verify they have exited before declaring generation stopped. The live inventory on October 28, 2026 found none in the scheduled/load-test clusters.

Signal has a separate stop branch based on `willpote/initial-signal`. Kailua launcher/watchdog changes have a separate branch based on `zeroecco/kailua_cleanup`; do not deploy the older Kailua project from main for this change.

Run `python3 infra/test-producer-stop.py og-service og-schedule load-test indexer` after installing each affected project's locked npm dependencies. The tests use Pulumi mocks to check generator stop/resume, the configured indexer alarm actions, and retention of resource identities and other inputs. They do not replace a live deployment preview.
