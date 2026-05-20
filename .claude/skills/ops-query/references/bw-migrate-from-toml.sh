#!/usr/bin/env bash
# One-shot migration from network_secrets.toml → Bitwarden items.
# Idempotent — re-running skips items that already exist.
#
# Requires: bw (unlocked), jq, python3 (3.11+ for tomllib).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./bw-credentials.sh
source "${SCRIPT_DIR}/bw-credentials.sh"

# Locate the TOML by walking up from cwd.
find_toml() {
  local dir="$PWD"
  while [ "$dir" != "/" ]; do
    if [ -f "${dir}/network_secrets.toml" ]; then
      echo "${dir}/network_secrets.toml"
      return 0
    fi
    dir="$(dirname "$dir")"
  done
  return 1
}

TOML="$(find_toml)" || {
  echo "network_secrets.toml not found in any parent of $PWD" >&2
  exit 1
}
echo "Reading: $TOML"

command -v python3 >/dev/null 2>&1 || {
  echo "python3 not found — required to parse TOML" >&2
  exit 1
}
command -v jq >/dev/null 2>&1 || {
  echo "jq not found — required to build payloads" >&2
  exit 1
}

bw_ensure_ready || exit 1

# Convert the TOML to JSON via stdlib tomllib (Python 3.11+).
TOML_JSON="$(python3 - "$TOML" <<'PY'
import json, sys, tomllib
with open(sys.argv[1], "rb") as f:
    print(json.dumps(tomllib.load(f)))
PY
)" || {
  echo "Failed to parse $TOML — needs Python 3.11+ (tomllib)" >&2
  exit 1
}

CREATED=0
SKIPPED=0

item_exists() {
  # `bw get item <name>` does fuzzy substring matching, which causes false
  # positives across similarly-named items (e.g. prod_base vs
  # prod_base_sepolia). Filter the search results by exact name in jq.
  local name="$1" count
  count="$(bw list items --search "$name" 2>/dev/null \
    | jq --arg n "$name" '[.[] | select(.name == $n)] | length')"
  [ "${count:-0}" -ge 1 ]
}

# Build a Bitwarden Login-item JSON payload.
# Args: name, username, password, [field_name field_value]...
login_payload() {
  local name="$1" username="$2" password="$3"
  shift 3
  local fields="[]"
  while [ $# -gt 0 ]; do
    fields=$(jq -c --arg n "$1" --arg v "$2" '. + [{name: $n, value: $v, type: 0}]' <<<"$fields")
    shift 2
  done
  jq -n \
    --arg name "$name" \
    --arg u "$username" \
    --arg p "$password" \
    --argjson fields "$fields" \
    '{
      organizationId: null,
      folderId: null,
      type: 1,
      name: $name,
      notes: null,
      favorite: false,
      fields: $fields,
      login: {username: $u, password: $p, uris: [], totp: null},
      secureNote: null,
      card: null,
      identity: null,
      reprompt: 0
    }'
}

create_item() {
  local name="$1" payload="$2"
  if item_exists "$name"; then
    echo "  exists, skipping: $name"
    SKIPPED=$((SKIPPED + 1))
    return 0
  fi
  echo "$payload" | bw encode | bw create item >/dev/null
  echo "  created: $name"
  CREATED=$((CREATED + 1))
}

# ── AWS ──────────────────────────────────────────────────────────────────────
for tier in $(jq -r '.aws // {} | keys[]' <<<"$TOML_JSON"); do
  name="boundless-ops-aws-${tier}"
  echo "AWS: $name"
  ak="$(jq -r --arg t "$tier" '.aws[$t].access_key_id' <<<"$TOML_JSON")"
  sk="$(jq -r --arg t "$tier" '.aws[$t].secret_access_key' <<<"$TOML_JSON")"
  create_item "$name" "$(login_payload "$name" "$ak" "$sk")"
done

# ── Indexer ──────────────────────────────────────────────────────────────────
for env in $(jq -r '.environments // {} | to_entries[] | select(.value.indexer) | .key' <<<"$TOML_JSON"); do
  name="boundless-ops-indexer-${env}"
  echo "Indexer: $name"
  api_key="$(jq -r --arg e "$env" '.environments[$e].indexer.api_key' <<<"$TOML_JSON")"
  chain="$(jq -r --arg e "$env" '.environments[$e].chain // ""' <<<"$TOML_JSON")"
  environment="$(jq -r --arg e "$env" '.environments[$e].environment // ""' <<<"$TOML_JSON")"
  create_item "$name" "$(login_payload "$name" "" "$api_key" chain "$chain" environment "$environment")"
done

# ── Telemetry ────────────────────────────────────────────────────────────────
for env in $(jq -r '.environments // {} | to_entries[] | select(.value.telemetry) | .key' <<<"$TOML_JSON"); do
  name="boundless-ops-telemetry-${env}"
  echo "Telemetry: $name"
  db_url="$(jq -r --arg e "$env" '.environments[$e].telemetry.db_url' <<<"$TOML_JSON")"
  pw="$(jq -r --arg e "$env" '.environments[$e].telemetry.readonly_password' <<<"$TOML_JSON")"
  chain="$(jq -r --arg e "$env" '.environments[$e].chain // ""' <<<"$TOML_JSON")"
  environment="$(jq -r --arg e "$env" '.environments[$e].environment // ""' <<<"$TOML_JSON")"
  create_item "$name" "$(login_payload "$name" "readonly" "$pw" db_url "$db_url" chain "$chain" environment "$environment")"
done

echo
echo "Summary: created $CREATED, skipped $SKIPPED (already exists)"
