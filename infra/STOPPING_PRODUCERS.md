# Stop request producers

Deploy this branch through the existing order-generator and indexer pipelines. Complete the indexer production update before approving the generator production update, so low-traffic alarms are muted first. Do not create missing stacks.

`order-generator-base:STOPPED: true` scales generator services to zero, disables random-requestor schedules, blocks load-test Lambda invocation with reserved concurrency zero, and mutes generator alarm actions. Resources, logs, secrets and pipelines remain managed.

The protocol sunset now disables all indexer alarm actions, including customer, market and service alarms. Metrics and alarm state continue updating.

To resume, set the generator flag to false and deploy. Restoring alerts also requires reverting the sunset-wide alarm disablement and the 19 producer/volume overrides in `infra/indexer/alarmConfig.ts`. Existing desired counts still apply, including zero counts on previously stopped networks.

Already running standalone tasks and in-flight Lambda invocations are not cancelled by these settings. Verify they have exited before declaring generation stopped. The live inventory on October 28, 2026 found none in the scheduled/load-test clusters.

Signal has a separate stop branch based on `willpote/initial-signal`. Kailua launcher/watchdog changes have a separate branch based on `zeroecco/kailua_cleanup`; do not deploy the older Kailua project from main for this change.

Run `python3 infra/test-producer-stop.py og-service og-schedule load-test indexer` after installing each affected project's locked npm dependencies. The tests use Pulumi mocks to check generator stop/resume, the configured indexer alarm actions, and retention of resource identities and other inputs. They do not replace a live deployment preview.

## Retire the dedicated service stacks

Keep the public explorer, all eight market/rewards indexer stacks, APIs, databases, caches, RPC access, shared bootstrap networking, and the indexer pipeline through 30 November 2026. This change does not retire any of those resources. Bento execution only computes optional cycle counts; losing it does not stop chain indexing.

`RETIRED: true` makes distributor, slasher, order-stream, order-generator, requestor-lists and the old Bento `cw-monitoring` programs register no service resources. Normal pipeline updates delete their dedicated infrastructure without building container images. The requestor lists are not an indexer dependency; existing external brokers configured to fetch them will lose those endpoints. The four retired service images are also removed from Docker CI.

All 24 launch service configs, both requestor-list configs, both old monitoring configs and eleven legacy distributor/order-stream configs are marked retired. On 12 November, the ten legacy checkpoints were empty; `order-stream/test` had no checkpoint. Do not create missing stacks. Empty checkpoints with pending operations are not confirmed clean until those operations are reconciled.

### Deployment order

1. Keep the wallet backup outside this repository. No other service data needs retaining.
2. Deploy and verify the indexer through its existing pipeline. Its `ORDER_STREAM_ENABLED` output must be false before order-stream retirement. This guard remains because order-stream polling differs from optional Bento execution.
3. Reconcile pending operations, then prepare the nonempty order-stream and requestor-list stacks with `prepare-retirement.py` before their normal pipeline updates. Review fresh deletion previews and use the usual production approvals. Other service stacks need preparation only if they still contain protected resources.
4. Run the old `l-prover-ansible` pipeline while it still exists. Its prover entry point now stops/disables Bento and Vector, removes their Compose containers and removes the Bento unit on inventory hosts. Monitoring and firewall entry points no longer reconfigure hosts. There is no execution/indexer guard and no image build. Host volumes remain. The Latitude owner has separately scheduled the execution hosts' removal for approximately 16 November; this playbook does not cancel leases. Run it before that removal if the hosts are still reachable. Review the separate CloudWatch retirement plans through this pipeline's existing staging/production stages.
5. Only after service and monitoring retirement has finished, preview and apply the `pipelines` ops stack. Its new program removes the seven obsolete service/KOG/Ansible pipelines and their dedicated build/notification resources. It retains the indexer pipeline, shared IAM, artifact/state buckets, KMS and notifications. Applying it earlier would remove the pipelines needed for steps 2–4. It also avoids redeploying the stale KOG definition from `main`; KOG retirement already completed from its owning branch.
6. After obsolete pipelines are gone, remove confirmed-empty retired checkpoints with the helper's `--remove-empty` option. Keep the retired config files so old stacks cannot be recreated accidentally.

No live preparation, refresh, deployment, stack removal or AWS deletion was run while preparing this PR. Staging deployments must use the existing pipeline, not local applies.

### Prepare deletion settings

The existing deployment state controls delete-time behavior even when a retired program returns no resources. From the appropriate `infra/<service>` directory, use:

```bash
AWS_PROFILE=boundless-ops python3 ../prepare-retirement.py <service> <stack>
```

The helper checks the retired flag, known stack/config, checkpoint ownership and pending operations. For order-stream it also checks the matching deployed indexer's output. It imports an encrypted checkpoint with protection removed, CloudWatch `skipDestroy` disabled, Redshift final snapshots disabled, and requestor-list bucket `forceDestroy` enabled. This prevents retaining logs/snapshots or failing on extra bucket objects. It does not delete AWS resources. Exported secrets stay encrypted and the private temporary file is removed on exit.

Prepare all four nonempty launch order-stream stacks, not only the two protected production stacks. Prepare requestor-lists `l-prod` and `l-staging` too. Do not run the helper concurrently with a pipeline or deploy an older revision between preparation and retirement. The normal pipeline refresh preserves these delete-only settings.

After a stack has no service resources or pending operations, the same helper can remove its empty checkpoint:

```bash
AWS_PROFILE=boundless-ops python3 ../prepare-retirement.py <service> <stack> --remove-empty
```

This mode refuses indexer/bootstrap/pipeline stacks and any checkpoint that still owns a non-Pulumi resource. It never forces stack removal.

### Unmanaged leftovers

The active execution log groups use older names outside `cw-monitoring` ownership: `/boundless/bento/explorer-8453-prod-release` in production and `/boundless/bento/execution-84532-staging-release` in staging. Delete these exact groups only after Vector/hosts stop, using the normal reviewed AWS cleanup. They are not removed by the Pulumi monitoring plan. Previously orphaned logs or snapshots also need explicit inventory and deletion; the helper changes only resources still owned by the selected stack.

The current HLC prover and Signal have separate ownership/deployment paths. This PR retires the legacy Bento inventory and does not claim to cancel unrelated leases or retire another repository's deployment.

### Validation

Run `python3 infra/test-retirement.py` after installing the service packages, plus `python3 ansible/tests/test_retirement.py` and Ansible lint. Tests cover retired programs, post-deploy behavior, indexer preservation, delete-only state settings, pending-operation refusal and safe empty-stack removal. Copied-state previews model the required indexer output and reconciled pending operations; they do not establish live health or resolve cloud drift.

On 12 November, all 25 nonempty-stack previews passed with Pulumi 3.193.0 and the locked AWS provider versions. They plan 292 distributor, 104 slasher, 186 order-stream, 681 order-generator, 26 requestor-list, 306 old-monitoring and 99 pipeline deletions, including components. No deletion overlaps the 750 retained resource identities checked. There are no creates or replacements. The pipeline preview also updates the unchanged shared notification Lambda bundle; its eight tests pass. Fourteen existing empty checkpoints have no resources to preview, and the legacy `test` checkpoint is absent.
