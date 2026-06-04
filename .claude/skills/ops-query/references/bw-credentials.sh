# Bitwarden-backed credential helpers for the ops-* skills.
#
# Usage from any ops-* skill:
#   source .claude/skills/ops-query/references/bw-credentials.sh
#   bw_ensure_ready || exit 1
#   bw_load_aws prod                  # or staging | dev | ops
#   bw_load_indexer prod_base         # exports INDEXER_API_KEY (optional)
#   bw_load_redshift_url prod_base    # exports REDSHIFT_URL
#
# Item schema is documented in bw-credentials.md.
#
# All lookups go through `bw_get_by_name` because `bw get item <name>` does
# fuzzy substring matching — `boundless-ops-indexer-prod_base` would also
# match `boundless-ops-indexer-prod_base_sepolia`. We require an exact match.

# Verify bw is installed and the vault is unlocked. Uses `bw status` (JSON)
# rather than a `bw list` probe because a locked vault makes `bw` prompt for
# the master password interactively; with stdin closed the readline call
# crashes but `bw` still exits 0, so the probe would falsely report ready.
# `bw status` is reliable on the pinned 2026.2.0; the regression referenced
# in GH bitwarden/clients#20703 only affects 2026.3.x / 2026.4.x, which this
# setup explicitly refuses to support.
bw_ensure_ready() {
  if ! command -v bw >/dev/null 2>&1; then
    echo "bw not installed. Pin to 2026.2.0 (Homebrew ships a broken release):" >&2
    echo "  npm install -g @bitwarden/cli@2026.2.0" >&2
    echo "Then: bw login" >&2
    return 1
  fi
  # Note: zsh reserves `status` as a read-only alias for $?, so use a
  # different name when sourcing in either bash or zsh.
  local vault_state; vault_state="$(bw status 2>/dev/null | jq -r '.status // "unknown"')"
  case "$vault_state" in
    unlocked) return 0 ;;
    locked|unauthenticated) ;;
    *) echo "bw status returned '$vault_state'. Try \`bw login\` in a terminal and re-run." >&2; return 1 ;;
  esac

  # Locked and unauthenticated share the same recovery flow; the only
  # difference is whether the user needs `bw login` first.
  # Note: a no-restart "paste !export BW_SESSION=… into the prompt" path
  # does NOT work here. Claude Code's Bash tool spawns a fresh subprocess
  # for each call, so an export from the prompt's shell isn't visible to
  # subsequent tool calls. BW_SESSION must be in the parent shell's
  # environment when `claude` launches, so the subprocesses inherit it.
  local prefix=""
  if [ "$vault_state" = "unauthenticated" ]; then
    echo "Bitwarden is not logged in. Exit Claude Code, then in the same shell run:" >&2
    prefix="bw login && "
  else
    echo "Bitwarden vault is locked. Exit Claude Code, then in the same shell run:" >&2
  fi
  echo "  ${prefix}export BW_SESSION=\"\$(bw unlock --raw)\"" >&2
  echo "Re-launch \`claude\` so the child process inherits BW_SESSION." >&2
  return 1
}

# Fetch a single item by exact name. Echoes the item JSON on stdout.
# Returns non-zero if zero or multiple items match.
bw_get_by_name() {
  local name="$1" items count id
  items="$(bw list items --search "$name" 2>/dev/null)" || return 1
  count="$(jq --arg n "$name" '[.[] | select(.name == $n)] | length' <<<"$items")"
  case "${count:-0}" in
    0) echo "No Bitwarden item named '$name'" >&2; return 1 ;;
    1) ;;
    *) echo "Multiple Bitwarden items named '$name' ($count) — please dedupe" >&2; return 1 ;;
  esac
  id="$(jq -r --arg n "$name" '.[] | select(.name == $n) | .id' <<<"$items")"
  bw get item "$id"
}

# Read a named custom field from a Bitwarden item.
bw_field() {
  local item; item="$(bw_get_by_name "$1")" || return 1
  jq -r --arg f "$2" '.fields[]? | select(.name==$f) | .value' <<<"$item"
}

# Export AWS credentials for a tier (prod | staging | dev | ops).
bw_load_aws() {
  local tier="$1" item
  item="$(bw_get_by_name "boundless-ops-aws-${tier}")" || return 1
  AWS_ACCESS_KEY_ID="$(jq -r '.login.username // ""' <<<"$item")"
  AWS_SECRET_ACCESS_KEY="$(jq -r '.login.password // ""' <<<"$item")"
  if [ -z "$AWS_ACCESS_KEY_ID" ] || [ -z "$AWS_SECRET_ACCESS_KEY" ]; then
    echo "Missing username/password on boundless-ops-aws-${tier}" >&2
    return 1
  fi
  export AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY
  export AWS_DEFAULT_REGION="us-west-2"
}

# Export INDEXER_API_KEY for an env. No-op if the item doesn't exist
# (the indexer works without a key, just with lower rate limits).
bw_load_indexer() {
  local item key
  item="$(bw_get_by_name "boundless-ops-indexer-$1" 2>/dev/null)" || return 0
  key="$(jq -r '.login.password // ""' <<<"$item")"
  [ -n "$key" ] && export INDEXER_API_KEY="$key"
}

# Build and export REDSHIFT_URL for an env.
bw_load_redshift_url() {
  local env="$1" item db_url pw
  item="$(bw_get_by_name "boundless-ops-telemetry-${env}")" || return 1
  db_url="$(jq -r '.fields[]? | select(.name=="db_url") | .value' <<<"$item")"
  pw="$(jq -r '.login.password // ""' <<<"$item")"
  if [ -z "$db_url" ] || [ -z "$pw" ]; then
    echo "boundless-ops-telemetry-${env} missing db_url field or password" >&2
    return 1
  fi
  export REDSHIFT_URL="postgres://readonly:${pw}@${db_url}"
}
