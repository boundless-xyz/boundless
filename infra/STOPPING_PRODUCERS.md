# Stop request producers

Deploy this branch through the existing order-generator and indexer pipelines. Complete the indexer production update before approving the generator production update, so low-traffic alarms are muted first. Do not create missing stacks.

`order-generator-base:STOPPED: true` scales generator services to zero, disables random-requestor schedules, blocks load-test Lambda invocation with reserved concurrency zero, and mutes generator alarm actions. Resources, logs, secrets and pipelines remain managed.

The protocol sunset now disables all indexer alarm actions, including customer, market and service alarms. Metrics and alarm state continue updating.

To resume, set the generator flag to false and deploy. Restoring alerts also requires reverting the sunset-wide alarm disablement and the 19 producer/volume overrides in `infra/indexer/alarmConfig.ts`. Existing desired counts still apply, including zero counts on previously stopped networks.

Already running standalone tasks and in-flight Lambda invocations are not cancelled by these settings. Verify they have exited before declaring generation stopped. The live inventory on October 28, 2026 found none in the scheduled/load-test clusters.

Signal has a separate stop branch based on `willpote/initial-signal`. Kailua launcher/watchdog changes have a separate branch based on `zeroecco/kailua_cleanup`; do not deploy the older Kailua project from main for this change.

Run `python3 infra/test-producer-stop.py og-service og-schedule load-test indexer` after installing each affected project's locked npm dependencies. The tests use Pulumi mocks to check generator stop/resume, the configured indexer alarm actions, and retention of resource identities and other inputs. They do not replace a live deployment preview.

## Retire the dedicated service stacks

Keep the explorer, market/rewards indexers, indexer APIs, their databases and caches, shared bootstrap networking, and staking dependencies through 30 November 2026. Staking dependencies may remain after that date. The indexer stack now reports `ORDER_STREAM_ENABLED` and `BENTO_EXECUTION_ENABLED`; both are false after its optional order-stream and Bento connections are removed.

`RETIRED: true` makes the distributor, slasher, order-stream and order-generator programs register no service resources. Their normal pipeline update then deletes their dedicated infrastructure, including databases, telemetry, storage and secrets. It does not build or look up container images. Shared stacks and the indexer are outside this retirement path.

The 24 launch configs set retirement true. Before applying, verify that the wallet backup is safely stored. Deploy the pipeline update first, then deploy the indexer and verify it is healthy. Both the retirement helper and the order-stream program refuse retirement unless the deployed indexer reports `ORDER_STREAM_ENABLED: false`. Review each deletion preview and approve production through the existing pipeline. Do not create missing stacks or run local staging applies. Keep retired configs in place so later pipeline runs cannot recreate these services.

The helper removes deletion protection only from a retired, allowlisted service's own launch stack. Running standalone tasks must finish or be stopped before deleting their cluster. Existing provider settings may retain CloudWatch log groups and create a final Redshift snapshot; account for those in the deletion preview and subsequent cleanup.

The order-stream post-deploy hook skips Redshift migrations when `RETIRED` is true, before reading database passwords. Active stacks still run their migrations.

### Read-only preview review, 10 November 2026

Pulumi 3.193.0 previews against local copies of all 24 launch checkpoints plan 292 distributor, 104 slasher, 186 order-stream and 681 order-generator resource deletions, including component resources. No planned deletion overlaps the 1,242 resource identities checked in retained indexer, bootstrap, requestor-list, monitoring and pipeline stacks. The four empty slasher/order-stream checkpoints for `l-prod-84532` and `l-prod-11155111` have nothing to retire; skip them. No live checkpoint or resource was changed.

These previews disabled protection only in the local copies and modeled the indexer reporting `ORDER_STREAM_ENABLED: false`. The deployed indexers do not yet have that output. Deploy and verify the indexer before retiring order-stream. Checkpoints also contain unfinished creates in slasher `l-staging-84532`, order-stream `l-prod-8453`, `l-prod-84532` and `l-prod-11155111`, and several indexer stacks. The normal pipeline refresh must reconcile these, followed by fresh deletion previews before approval. The copied-state previews do not verify cloud drift or indexer health. Order-stream log groups remain because of `skipDestroy`; its Redshift clusters request final snapshots.

This branch covers the four service projects above. Signal, the live Kailua deployment, leased prover machines and their external restart/deployment jobs need retirement in their owning repositories; do not deploy the old Kailua project from this branch. Remove obsolete service pipelines after their stacks have been retired.

Run `python3 infra/test-retirement.py` with the service projects' npm dependencies available to test the deletion boundaries and empty retired programs without cloud changes.
