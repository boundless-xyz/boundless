---
name: ops-pipelines
description: >-
  Internal — for Boundless team members only. Monitor Boundless deployment
  pipelines (AWS CodePipeline + CodeBuild) on the ops account. Use when the
  user wants to track a deployment after merging a PR, check whether a commit
  has rolled out to staging/prod, diagnose a failed deployment, watch the
  status of a specific pipeline, or get prompted to approve a production
  rollout once staging succeeds. Do NOT use for service runtime debugging
  (use ops-logs-query) or for deploying dev infrastructure (use ops-infra-deploy).
---

# Ops Pipelines

Monitor Boundless service deployments running through AWS CodePipeline /
CodeBuild in the ops account. Designed for the post-merge workflow: track a
commit through staging, surface failures with build logs, and prompt the user
to approve the production rollout.

## Setup

Read `network_secrets.toml` from the repo root and extract `[aws.ops]`
(`access_key_id`, `secret_access_key`). These are read-only and can query
CodePipeline / CodeBuild / CloudWatch but cannot approve, start, or retry
pipelines. If the file is missing, point the user at the **Boundless
runbook**.

```bash
export AWS_ACCESS_KEY_ID="..."
export AWS_SECRET_ACCESS_KEY="..."
export AWS_DEFAULT_REGION="us-west-2"
```

All Boundless pipelines live in `us-west-2` in account `968153779208`
(BoundlessOps).

## Pipelines

Pipeline definitions live in `infra/pipelines/pipelines/` — read that dir to
see what's deployed, branch config, stage layout per service. The boundless
repo's pipelines are the `l-*` ones; `signal-pipeline`,
`kailua-order-generator-pipeline`, `zeth-requestor-pipeline` come from
different repos.

Discover the live list any time:

```bash
aws codepipeline list-pipelines --query 'pipelines[].name' --output table
```

Typical stage layout: `Source` → `DeployStaging` (parallel CodeBuild per
chain) → `DeployProduction` (manual approval, then parallel CodeBuild per
chain). `l-prover-ansible-pipeline` adds a `DeployNightly` stage between
staging and production.

CodeBuild project names have a Pulumi resource hash suffix
(`l-indexer-staging-167000-build-aee3645`); always derive the suffix from
pipeline state, never hardcode it.

## Core workflows

### 1. Monitor a deployment for a freshly merged commit

Primary use case. Given a commit SHA (or PR), find the pipeline executions
for that SHA and poll until staging finishes.

Resolve the SHA if needed:

```bash
gh pr view <number> --json mergeCommit --jq '.mergeCommit.oid'
```

Find the matching execution per pipeline (executions store the SHA in
`sourceRevisions[0].revisionId`):

```bash
SHA="..."
for P in $(aws codepipeline list-pipelines --query 'pipelines[?starts_with(name, `l-`)].name' --output text); do
  EXEC=$(aws codepipeline list-pipeline-executions --pipeline-name "$P" --max-items 20 \
    --query "pipelineExecutionSummaries[?sourceRevisions[0].revisionId=='$SHA'] | [0].pipelineExecutionId" \
    --output text)
  echo "$P -> $EXEC"
  sleep 1
done
```

Default to all `l-*` pipelines unless the user specifies a service. Skip
`l-prover-ansible-pipeline` unless the change touched `ansible/` or
`infra/cw-monitoring/`.

If the SHA's execution is `Superseded`, a later commit took over and that
SHA will not deploy to prod on its own. Call this out — common source of
confusion when merging PRs back-to-back.

**Use the Monitor tool to poll in the background** so the user can keep
working while staging runs (CodePipeline deployments take 10–30+ minutes).
The Monitor tool runs a script in the background and feeds each output line
back, so the agent can interject as soon as a stage transitions.

Run one Monitor per tracked pipeline with a script that prints a status
heartbeat every 30s and exits on a terminal event. Use the execution's
overall status from `get-pipeline-execution`, and `get-pipeline-state` to
detect the approval gate (filtered to the inbound exec at DeployProduction
so a newer superseding exec doesn't trigger a false approval signal):

```bash
PIPELINE="l-indexer-pipeline"
EXEC="<pipelineExecutionId from step above>"
while true; do
  STATUS=$(aws codepipeline get-pipeline-execution \
    --pipeline-name "$PIPELINE" --pipeline-execution-id "$EXEC" \
    --query 'pipelineExecution.status' --output text 2>/dev/null || echo Unknown)
  echo "$(date -u +%H:%M:%SZ) $PIPELINE exec=$EXEC status=$STATUS"
  case "$STATUS" in
    Succeeded)
      echo "DONE pipeline=$PIPELINE exec=$EXEC"; break ;;
    Failed|Stopped|Cancelled|Superseded)
      echo "ALERT pipeline=$PIPELINE status=$STATUS exec=$EXEC"; break ;;
    InProgress)
      APPROVAL=$(aws codepipeline get-pipeline-state --name "$PIPELINE" --output json \
        | jq -r --arg E "$EXEC" '
          .stageStates[]
          | select(.stageName=="DeployProduction"
                   and .inboundExecution.pipelineExecutionId == $E)
          | .actionStates[] | select(.actionName=="ApproveDeployToProduction")
          | .latestExecution.status // empty' | head -1)
      if [ "$APPROVAL" = "InProgress" ]; then
        echo "READY-TO-APPROVE pipeline=$PIPELINE exec=$EXEC"; break
      fi ;;
  esac
  sleep 30
done
```

Each tracked pipeline gets its own Monitor (run them in parallel — the
boundless ops account handles the call rate fine at 30s intervals). React
when a line starting with `ALERT`, `READY-TO-APPROVE`, or `DONE` arrives:

- `READY-TO-APPROVE` → prompt the user to approve production (workflow 2).
- `ALERT ... Failed` → surface the failure (workflow 3).
- `ALERT ... Superseded` → a newer commit took over; tell the user this SHA
  will not deploy to prod on its own.
- `DONE ... Succeeded` → pipeline fully complete (rare without approval).

If the user cancels the run, moves on to unrelated work, or asks to stop
monitoring, cancel the monitors — they cost API calls and clutter context.
If the Monitor tool isn't available, fall back to manual polling with
`get-pipeline-state` every 30s.

### 2. Approving production deploys

When staging completes, summarise: commit SHA + subject, which pipelines are
waiting, and per-pipeline AWS Console links:

```
https://us-west-2.console.aws.amazon.com/codesuite/codepipeline/pipelines/<pipeline-name>/view?region=us-west-2
```

Ask the user explicitly whether to approve. The user approves via the AWS
Console (or Slack — pipelines emit `manual-approval-needed` events to the
`boundless-alerts-launch` and `boundless-alerts-staging-launch` channels).

NEVER attempt the approval call yourself — the read-only ops creds will fail
with `AccessDenied`. After the user approves, optionally keep polling
production stages.

### 3. Diagnose a failed deployment

Find the failed action and its CodeBuild build:

```bash
aws codepipeline list-action-executions --pipeline-name "$P" \
  --filter pipelineExecutionId="$EXEC" \
  --query "actionExecutionDetails[?status=='Failed'].{stage:stageName, action:actionName, build:output.executionResult.externalExecutionId, project:input.configuration.ProjectName, url:output.executionResult.externalExecutionUrl}" \
  --output json
```

`build` is `<project>:<uuid>`. Get the failed phase + log location:

```bash
aws codebuild batch-get-builds --ids "$BUILD_ID" \
  --query 'builds[].{status:buildStatus, phase:currentPhase, group:logs.groupName, stream:logs.streamName, deepLink:logs.deepLink, start:startTime, end:endTime, failures:phases[?phaseStatus==`FAILED`].[phaseType,contexts[].message]}' \
  --output json
```

Pull log lines around the failure (CodeBuild logs go to
`/aws/codebuild/<project-name>`):

```bash
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --log-stream-names "$LOG_STREAM" \
  --start-time "$START_MS" --end-time "$END_MS" \
  --filter-pattern '?ERROR ?error ?Failed ?failed ?"exit code"' \
  --output json | jq '.events[] | {ts: (.timestamp/1000|todate), msg: .message}'
```

Common Boundless-specific failure patterns:

- **`Still using ops account`** — assume-role didn't take effect; usually
  transient, retry the stage.
- **`pulumi cancel` / `update is in progress`** — previous run was killed;
  next run usually self-recovers via `pulumi cancel --yes` in the buildspec.
- **`Resource ... already exists`** — Pulumi state drift; manual fix.
- **`401 Unauthorized` from ghcr.io / docker.io** — token rotation issue.
- **`AccessDenied`** — IAM problem in the target account.
- **`unhealthy` / `failed to start: container`** (prover-ansible) —
  cross-reference with `ops-logs-query` on the bento prover log group.

Surface the failed phase + 10–30 most relevant log lines + console deep
link. Don't dump the full build log.

### 4. Fleet status overview

```bash
for P in $(aws codepipeline list-pipelines --query 'pipelines[].name' --output text); do
  echo "=== $P ==="
  aws codepipeline get-pipeline-state --name "$P" \
    --query 'stageStates[].{stage:stageName, status:latestExecution.status}' \
    --output table
  sleep 1
done
```

Highlight: any `Failed` stage, any `DeployProduction` waiting on approval,
pipelines with no recent runs.

## Status reference

| Status                 | Meaning                                                                                              |
| ---------------------- | ---------------------------------------------------------------------------------------------------- |
| `InProgress`           | Currently running.                                                                                   |
| `Succeeded`            | Finished successfully.                                                                               |
| `Failed`               | Failed; pipeline halted.                                                                             |
| `Stopped` / `Stopping` | Manually stopped.                                                                                    |
| `Superseded`           | Newer execution took over; this one will not progress further. Common — webhook fires on every push. |
| `Cancelled`            | Cancelled before completion (rare).                                                                  |

CodeBuild: `SUCCEEDED`, `FAILED`, `FAULT`, `TIMED_OUT`, `IN_PROGRESS`,
`STOPPED`.

## Tips

- `sleep 1` between AWS calls — CodePipeline TPS limits are low.
- Prefer `get-pipeline-state` over `list-action-executions` for live polling
  (one call, everything needed).
- Always show full pipeline name + execution ID + console deep link in any
  status report.
- A stage can show `Failed` while most chain-specific actions inside it
  succeeded — identify which chain(s) actually failed.

## Important

- NEVER attempt to approve, start, retry, or stop a pipeline — the
  read-only ops creds fail with `AccessDenied`. Direct the user to the AWS
  Console.
- NEVER fabricate a `pipelineExecutionId`, `actionExecutionId`, approval
  token, or CodeBuild ID. They must come from a live AWS query.
- This skill is read-only. To modify pipelines themselves, see
  `infra/pipelines/`.
