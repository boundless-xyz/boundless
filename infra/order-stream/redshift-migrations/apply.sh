#!/usr/bin/env bash
set -euo pipefail

# Applies pending Redshift migrations in order.
# Fetches Kinesis stream names, Redshift endpoint, and IAM role ARN
# from Pulumi stack outputs automatically.
#
# Usage:
#   ./apply.sh <pulumi-stack>
#
# Required environment variables:
#   REDSHIFT_ADMIN_PASSWORD    - Admin password for the Redshift database
#   REDSHIFT_READONLY_PASSWORD - Password for the read-only database user
#
# Example:
#   export REDSHIFT_ADMIN_PASSWORD="admin-password"
#   export REDSHIFT_READONLY_PASSWORD="readonly-password"
#   ./apply.sh dev

STACK="${1:?Usage: $0 <pulumi-stack>}"

: "${REDSHIFT_ADMIN_PASSWORD:?REDSHIFT_ADMIN_PASSWORD must be set}"
: "${REDSHIFT_READONLY_PASSWORD:?REDSHIFT_READONLY_PASSWORD must be set}"

export PGPASSWORD="$REDSHIFT_ADMIN_PASSWORD"
export PGCONNECT_TIMEOUT=10

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INFRA_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "Fetching stack outputs from pulumi stack '$STACK'..."

get_output() {
    local val
    val=$(pulumi stack output "$1" --stack "$STACK" --cwd "$INFRA_DIR" 2>/dev/null || true)
    if [ -z "$val" ]; then
        echo "ERROR: Pulumi stack output '$1' is empty or not set." >&2
        exit 1
    fi
    echo "$val"
}

export KINESIS_HEARTBEAT_STREAM
KINESIS_HEARTBEAT_STREAM=$(get_output "KINESIS_HEARTBEAT_STREAM")

export KINESIS_EVALUATIONS_STREAM
KINESIS_EVALUATIONS_STREAM=$(get_output "KINESIS_EVALUATIONS_STREAM")

export KINESIS_COMPLETIONS_STREAM
KINESIS_COMPLETIONS_STREAM=$(get_output "KINESIS_COMPLETIONS_STREAM")

ENDPOINT=$(get_output "REDSHIFT_ENDPOINT")

export IAM_ROLE_ARN
IAM_ROLE_ARN=$(get_output "REDSHIFT_IAM_ROLE_ARN")

DATABASE="telemetry"
RS_USER="admin"
PORT="5439"

echo "Redshift endpoint: $ENDPOINT"
echo "Kinesis streams:   $KINESIS_HEARTBEAT_STREAM, $KINESIS_EVALUATIONS_STREAM, $KINESIS_COMPLETIONS_STREAM"
echo "IAM role:          $IAM_ROLE_ARN"

run_psql() {
    psql -h "$ENDPOINT" -p "$PORT" -U "$RS_USER" -d "$DATABASE" "$@"
}

echo "Testing connection..."
if ! run_psql -c "SELECT 1;"; then
    echo "ERROR: Cannot connect to Redshift at $ENDPOINT:$PORT" >&2
    echo "Check that your IP is allowed by the security group and the endpoint is reachable." >&2
    exit 1
fi
echo "Connected."

# Create or update the read-only user (Redshift has no IF NOT EXISTS for users)
if run_psql -t -A -c "SELECT 1 FROM pg_user WHERE usename='readonly';" | grep -q 1; then
    echo "Updating readonly user password..."
    run_psql -c "ALTER USER readonly PASSWORD '$REDSHIFT_READONLY_PASSWORD';"
else
    echo "Creating readonly user..."
    run_psql -c "CREATE USER readonly PASSWORD '$REDSHIFT_READONLY_PASSWORD';"
fi

# Ensure the _migrations table exists (handles first-ever run)
run_psql -c "CREATE SCHEMA IF NOT EXISTS telemetry;"
run_psql -c "CREATE TABLE IF NOT EXISTS telemetry._migrations (version INTEGER PRIMARY KEY, name VARCHAR(256) NOT NULL, applied_at TIMESTAMPTZ NOT NULL DEFAULT GETDATE());"

# Get the latest applied version
LATEST=$(run_psql -t -A -c "SELECT COALESCE(MAX(version), 0) FROM telemetry._migrations;")
echo "Latest applied migration: $LATEST"

APPLIED=0
for file in "$SCRIPT_DIR"/[0-9]*.sql; do
    [ -f "$file" ] || continue
    BASENAME="$(basename "$file")"
    VERSION=$(echo "$BASENAME" | grep -o '^[0-9]*' | sed 's/^0*//')

    if [ "$VERSION" -le "$LATEST" ]; then
        echo "Skipping $BASENAME (already applied)"
        continue
    fi

    echo "Applying $BASENAME ..."
    envsubst < "$file" | run_psql
    APPLIED=$((APPLIED + 1))
    echo "Applied $BASENAME"
done

if [ "$APPLIED" -eq 0 ]; then
    echo "No pending migrations."
else
    echo "Applied $APPLIED migration(s)."
fi
