#!/usr/bin/env bash
set -euo pipefail

# Runs Redshift migrations after a successful deployment.
# Used by the deployment pipelines to apply any new migrations post deploy.
# Extracts passwords from Pulumi config so no manual env vars are needed.
#
# Usage:
#   ./post-deploy.sh <pulumi-stack>

STACK="${1:?Usage: $0 <pulumi-stack>}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== Post-deploy: Redshift migrations ==="

# Install psql + envsubst if not present (CodeBuild image may not have them)
if ! command -v psql &>/dev/null || ! command -v envsubst &>/dev/null; then
    echo "Installing postgresql-client and gettext-base..."
    apt-get update -qq && apt-get install -y -qq postgresql-client gettext-base
fi

# Prefer env vars (dev stacks set them at deploy time); fall back to
# pulumi config (staging/prod store them as pulumi secrets).
export REDSHIFT_ADMIN_PASSWORD
if [ -z "${REDSHIFT_ADMIN_PASSWORD:-}" ]; then
    REDSHIFT_ADMIN_PASSWORD=$(pulumi config get REDSHIFT_ADMIN_PASSWORD --stack "$STACK" --cwd "$SCRIPT_DIR")
fi

export REDSHIFT_READONLY_PASSWORD
if [ -z "${REDSHIFT_READONLY_PASSWORD:-}" ]; then
    REDSHIFT_READONLY_PASSWORD=$(pulumi config get REDSHIFT_READONLY_PASSWORD --stack "$STACK" --cwd "$SCRIPT_DIR")
fi

# Run migrations
"$SCRIPT_DIR/redshift-migrations/apply.sh" "$STACK"

echo "=== Post-deploy complete ==="
