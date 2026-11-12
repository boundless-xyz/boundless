# Remaining market cleanup

This is a reviewed target list and a read-only preview tool. It does not remove
resources or change Pulumi state. Execute approved target cleanup separately.
The retirement playbook still follows its existing deployment pipeline; changes
to that playbook can advance the existing host and monitoring retirement.

```sh
python3 infra/cleanup/test_preview.py
python3 infra/cleanup/preview.py --output /private/tmp/market-cleanup-plan.json
```

The preview verifies the AWS accounts, exact resource identities, bucket
versioning/notifications, and the reviewed Pulumi checkpoint versions before
printing any deletion plan. Its only AWS calls are reads. The `steps` in the JSON
are proposed commands, not commands executed by the tool. Run it again before a
separately approved cleanup; changed resources or checkpoints require review.

Each command uses `aws --profile <entry.profile> --region us-west-2` followed by
the command arguments. A `wait_until` step means poll that read until the result
matches, before continuing. There is no automatic executor. The six copied-state
destroy previews and preservation checks are recorded in `preview-evidence.json`;
their counts include resources already absent from AWS.

## Included

- The dev `pote` slasher and `pote999` order stream, its database, telemetry
  warehouse/streams and load balancer, and two dev producer schedules.
- Five dated market telemetry archive buckets, three old order-stream RDS
  snapshots and two final Redshift snapshots.
- Six historical Pulumi stacks: the three `slasher/prod-*` stacks,
  `prover/staging-11155111`, `prover/staging-84532`, and
  `prover-cluster/staging-11155111`. Keep their storage and monitoring cleanup
  under Pulumi rather than deleting tracked resources through AWS separately.

Disable the two producer schedules first, then remove the dev services before
removing their database, telemetry and storage. The five archive buckets are unversioned;
preview rejects a bucket if versioning or notifications change. Bucket contents
and snapshots are disposable under the retirement decision.

The managed-stack previews use copied checkpoints. Before an approved destroy,
review live drift, delete protections and retained-resource settings again.
Preserve secrets until their wallet backup has been checked. Staging teardown
must use the normal reviewed CI workflow; do not apply it directly from a laptop.
No new execution workflow is added here.

## Kept out

Keep all non-target workloads and dependencies, including resources that appear
unused. Keep the six active indexer stacks, explorer,
bootstrap networking, shared roles/state/artifact storage, and their dependencies
through 30 November 2026.

Resources with uncertain ownership are out of scope. Paused workloads remain
untouched.

Production `cw-monitoring` already has a retirement program. The existing
`DeployNightly` stage applies it before the manual production approval gate.
Merging the Ansible fix can therefore stop reachable staging/nightly workloads
and delete production monitoring without that approval. Keep this PR in draft
until those actions are explicitly authorized or the deployed pipeline is changed.
Remove obsolete pipelines only after the remaining service and monitoring work is
complete.
