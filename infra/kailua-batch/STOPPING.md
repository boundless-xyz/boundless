# Retire Kailua infrastructure

This branch is based on `origin/zeroecco/kailua_cleanup`, which contains the deployed trigger and watchdog. Deploy through the existing Kailua pipeline using that deployment reference.

`kailua-batch:RETIRED: true` returns before reading service config, resolving the shared bootstrap stack or registering service resources. The normal pipeline update deletes the dedicated ECS cluster, task definition, trigger/watchdog Lambdas and schedules, IAM roles, logs, alarms and secrets. It does not build container images. The pipeline and shared bootstrap networking stay in place, as do the explorer and indexer. The legacy `kailua-order-generator-pipeline` in the Signal repository is outside this change.

Before deployment, verify the wallet backup and that no ECS task or Lambda invocation remains active. Review fresh deletion previews for `staging` and `prod`; deploy through the normal staging and production approval stages. Do not run a local apply. Secrets Manager recovery windows still apply to deleted secrets. Keep `RETIRED` true to prevent recreation.

Read-only checks on 10 November 2026 confirmed both wallet values are in the private backup and both ECS clusters have zero running or pending tasks. Pulumi 3.193.0 previews against copies of the deployed checkpoints plan 52 resource deletions per environment, including the task-definition component, with no creates, updates or overlap with retained infrastructure. Discarding the bootstrap stack reference does not delete bootstrap resources. Neither stack has protected resources or pending operations, so no unprotect helper is needed. Logs are deleted; the nine secrets per environment keep their existing 30-day recovery windows. All 17 checked live checkpoints remained unchanged. Recheck task activity and previews before deployment.

`STOPPED: true` remains configured for both stacks. Clearing `RETIRED` recreates resources in that paused state; clearing both flags allows the old controls to resume, subject to their existing zero production concurrency and schedule counts.

Run `python3 infra/test-producer-stop.py kailua` after installing the project's locked npm dependencies. The tests cover retirement without resource, config or secret access, plus the existing stop/resume and idle behavior. They do not replace a live deployment preview.
