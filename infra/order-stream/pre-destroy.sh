#!/usr/bin/env bash
set -euo pipefail

# Archives telemetry data to S3 as Parquet before destroying the Redshift
# cluster. Runs UNLOAD against the deduplicated typed views so the output
# is directly queryable with Athena.
#
# Usage:
#   TELEMETRY_ARCHIVE_BUCKET=<bucket> ./pre-destroy.sh <pulumi-stack>
#
# Optional env vars:
#   AWS_REGION                 - Defaults to us-west-2
#   REDSHIFT_ENDPOINT          - Overrides `pulumi stack output REDSHIFT_ENDPOINT`
#   REDSHIFT_IAM_ROLE_ARN      - Overrides `pulumi stack output REDSHIFT_IAM_ROLE_ARN`
#   REDSHIFT_ADMIN_PASSWORD    - Overrides `pulumi config get REDSHIFT_ADMIN_PASSWORD`
#
# The overrides let you run when the Pulumi backend and the target AWS account
# require different credentials. Read the outputs first with the Pulumi-state
# creds, export them, switch to the target-account creds, then run this script.
#
# Example split-credential flow:
#   # with Pulumi-state creds:
#   export REDSHIFT_ENDPOINT=$(pulumi stack output REDSHIFT_ENDPOINT --stack l-prod-8453)
#   export REDSHIFT_IAM_ROLE_ARN=$(pulumi stack output REDSHIFT_IAM_ROLE_ARN --stack l-prod-8453)
#   export REDSHIFT_ADMIN_PASSWORD=$(pulumi config get REDSHIFT_ADMIN_PASSWORD --stack l-prod-8453)
#   # swap to target-account creds:
#   export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
#   ./pre-destroy.sh l-prod-8453
#
# If you also want a Redshift snapshot as a belt-and-suspenders backup
# (restoreable, but not directly queryable), run separately after this:
#   aws redshift-serverless create-snapshot \
#     --namespace-name <namespace> \
#     --snapshot-name telemetry-pre-migration-<stack>-$(date +%Y%m%d) \
#     --retention-period -1

STACK="${1:?Usage: $0 <pulumi-stack>}"
BUCKET="${TELEMETRY_ARCHIVE_BUCKET:?TELEMETRY_ARCHIVE_BUCKET must be set}"
REGION="${AWS_REGION:-us-west-2}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== Pre-destroy: Redshift telemetry export to S3 ==="

# Install psql if not present (CodeBuild image may not have it)
if ! command -v psql &>/dev/null; then
    echo "Installing postgresql-client..."
    apt-get update -qq && apt-get install -y -qq postgresql-client
fi

# Stack outputs can come from env vars (for split-credential flows) or from
# `pulumi stack output` (requires the Pulumi-state creds to be active).
get_output() {
    local val
    val=$(pulumi stack output "$1" --stack "$STACK" --cwd "$SCRIPT_DIR" 2>/dev/null || true)
    if [ -z "$val" ]; then
        echo "ERROR: Pulumi stack output '$1' is empty or not set." >&2
        echo "       Set it via env var (e.g. export $1=...) if you are using split credentials." >&2
        exit 1
    fi
    echo "$val"
}

if [ -n "${REDSHIFT_ENDPOINT:-}" ] && [ -n "${REDSHIFT_IAM_ROLE_ARN:-}" ]; then
    echo "Using REDSHIFT_ENDPOINT + REDSHIFT_IAM_ROLE_ARN from env (skipping pulumi stack output)."
    ENDPOINT="$REDSHIFT_ENDPOINT"
    IAM_ROLE_ARN="$REDSHIFT_IAM_ROLE_ARN"
else
    echo "Fetching stack outputs from pulumi stack '$STACK'..."
    ENDPOINT=$(get_output "REDSHIFT_ENDPOINT")
    IAM_ROLE_ARN=$(get_output "REDSHIFT_IAM_ROLE_ARN")
fi
ROLE_NAME="${IAM_ROLE_ARN##*/}"
S3_PREFIX="s3://$BUCKET/$STACK"

echo "Endpoint:  $ENDPOINT"
echo "IAM role:  $IAM_ROLE_ARN"
echo "Archive:   $S3_PREFIX/"

# Create archive bucket if missing (idempotent)
if ! aws s3api head-bucket --bucket "$BUCKET" --region "$REGION" 2>/dev/null; then
    echo "Creating bucket s3://$BUCKET in $REGION..."
    aws s3api create-bucket \
        --bucket "$BUCKET" \
        --region "$REGION" \
        --create-bucket-configuration "LocationConstraint=$REGION"
fi

# Grant the Redshift IAM role permission to write to the archive bucket.
# put-role-policy is idempotent: re-runs overwrite the existing policy.
echo "Attaching UNLOAD policy to role $ROLE_NAME..."
aws iam put-role-policy \
    --role-name "$ROLE_NAME" \
    --policy-name telemetry-unload-s3 \
    --policy-document "{
        \"Version\": \"2012-10-17\",
        \"Statement\": [{
            \"Effect\": \"Allow\",
            \"Action\": [\"s3:PutObject\", \"s3:GetBucketLocation\", \"s3:ListBucket\"],
            \"Resource\": [
                \"arn:aws:s3:::$BUCKET\",
                \"arn:aws:s3:::$BUCKET/*\"
            ]
        }]
    }"

# Ensure the ephemeral policy is removed whether the script succeeds or fails.
cleanup_policy() {
    echo "Removing UNLOAD policy from role $ROLE_NAME..."
    aws iam delete-role-policy \
        --role-name "$ROLE_NAME" \
        --policy-name telemetry-unload-s3 \
        || echo "Warning: failed to delete policy telemetry-unload-s3; remove manually."
}
trap cleanup_policy EXIT

# IAM propagation can take a few seconds; Redshift may otherwise see
# AccessDenied on the first UNLOAD.
sleep 10

# Admin password is needed to run UNLOAD. Prefer env var (dev stacks read
# it from env at deploy time); fall back to pulumi config (staging/prod
# store it as a pulumi secret).
export PGPASSWORD
if [ -n "${REDSHIFT_ADMIN_PASSWORD:-}" ]; then
    PGPASSWORD="$REDSHIFT_ADMIN_PASSWORD"
else
    PGPASSWORD=$(pulumi config get REDSHIFT_ADMIN_PASSWORD --stack "$STACK" --cwd "$SCRIPT_DIR")
fi
export PGCONNECT_TIMEOUT=10

run_psql() {
    psql -h "$ENDPOINT" -p 5439 -U admin -d telemetry -v ON_ERROR_STOP=1 "$@"
}

echo "Verifying connection..."
run_psql -c "SELECT 1;" >/dev/null

unload_view() {
    local view="$1"
    echo "Unloading telemetry.$view -> $S3_PREFIX/$view/"
    run_psql <<SQL
UNLOAD ('SELECT * FROM telemetry.$view')
TO '$S3_PREFIX/$view/'
IAM_ROLE '$IAM_ROLE_ARN'
FORMAT AS PARQUET
ALLOWOVERWRITE;
SQL
}

unload_view broker_heartbeats
unload_view request_evaluations
unload_view request_completions

echo ""
echo "Archive contents:"
aws s3 ls "$S3_PREFIX/" --recursive --summarize --human-readable | tail -20

echo ""
echo "=== Pre-destroy complete ==="
echo "Archive: $S3_PREFIX/"
echo "Query with Athena by creating external tables over this prefix."
