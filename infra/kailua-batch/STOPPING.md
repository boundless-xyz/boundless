# Stop Kailua launchers

This branch is based on `origin/zeroecco/kailua_cleanup`, which contains the deployed trigger and watchdog. Deploy through the existing Kailua pipeline using that deployment reference.

`kailua-batch:STOPPED: true` disables both schedules, sets both Lambdas to reserved concurrency zero, and mutes Kailua alarm actions. Tasks, logs, secrets and pipelines remain managed. Verify no standalone task or in-flight invocation remains before declaring the stop complete.

Set `STOPPED` to false and deploy to restore the prior controls. The existing concurrency and schedule-count settings still apply. Production already had generation disabled through those settings, so removing this stop flag alone does not resume production generation.

Run `python3 infra/test-producer-stop.py kailua` after installing each affected project's locked npm dependencies. The tests use Pulumi mocks to check stopping, resuming and retention of resource identities and other inputs. They do not replace a live deployment preview.
