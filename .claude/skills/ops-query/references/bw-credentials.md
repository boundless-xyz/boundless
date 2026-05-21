# Bitwarden credentials for the ops-* skills

All credentials used by `ops-query`, `ops-indexer-query`, `ops-telemetry-query`, `ops-logs-query`, and `ops-pipelines` live in Bitwarden — one item per credential, looked up by exact name. There is no plaintext fallback on disk; if `bw` is missing or the vault is locked, the skills fail loud.

## One-time setup

1. **Install the CLI — pin v2026.2.0.** Newer versions (`2026.3.0`, `2026.4.1`) have a regression that breaks non-interactive use: see [Known issues](#known-issues).

   ```bash
   npm install -g @bitwarden/cli@2026.2.0
   bw --version   # → 2026.2.0
   ```

   If you already have a newer `bw` installed (e.g. via Homebrew), remove it first:

   ```bash
   bw logout 2>/dev/null
   brew uninstall bitwarden-cli 2>/dev/null
   rm -rf "$HOME/Library/Application Support/Bitwarden CLI/"
   ```

2. **Log in.**

   ```bash
   bw login
   ```

   On macOS the encrypted local vault lives at `~/Library/Application Support/Bitwarden CLI/`. You only do this once per machine.

## Per-shell setup

Unlock the vault and export the session token before running any ops-* skill:

```bash
export BW_SESSION="$(bw unlock --raw)"
```

The session lives for the rest of that shell (configurable via `bw config server` / Bitwarden settings). All `bw get …` calls hit the encrypted local cache — no network round trip per read.

Verify with:

```bash
bw status | jq -r .status   # → "unlocked"
```

## Sourcing pattern

Every ops-* skill begins its credential step with:

```bash
source .claude/skills/ops-query/references/bw-credentials.sh
bw_ensure_ready || exit 1
```

Then it pulls only the credentials it needs:

```bash
bw_load_aws prod                    # AWS keys for the prod tier
bw_load_indexer prod_base           # INDEXER_API_KEY (optional)
bw_load_redshift_url prod_base      # REDSHIFT_URL
```

See `bw-credentials.sh` for the exact functions.

## Bootstrap

The items live in a **shared Bitwarden organization/collection** so the team only seeds them once. Two paths:

- **Joining the team:** an admin grants you access to the collection. After `bw login` + `bw sync`, the items show up in your local vault — no further setup needed.
- **One-time seeding (someone has to do this once):** with a local `network_secrets.toml` in hand, run the one-shot migration:

  ```bash
  bash .claude/skills/ops-query/references/bw-migrate-from-toml.sh
  ```

  It creates 17 items (8 indexer, 6 telemetry, 3 AWS: `prod`, `staging`, `dev`) into your personal vault. Move them into the shared collection via the Bitwarden web UI so the rest of the team gets them.

  The 18th item — `boundless-ops-aws-ops` (BoundlessOps account, read-only) — is added directly to the shared collection by an admin from runbook credentials; the TOML does not contain it. Use a Login item with `username` = `AWS_ACCESS_KEY_ID` and `password` = `AWS_SECRET_ACCESS_KEY`.

Verify with `bw list items --search boundless-ops- | jq '.[].name' | wc -l` — the expected count is 18.

The full item schema (exact field names per item type) lives in [`bw-migrate-from-toml.sh`](bw-migrate-from-toml.sh) and the loader functions in [`bw-credentials.sh`](bw-credentials.sh) — those are the source of truth.

## Troubleshooting

- **`bw_ensure_ready` says "vault is locked"** — exit Claude Code, then in the same shell run `export BW_SESSION="$(bw unlock --raw)"` and re-launch `claude`. The child process inherits `BW_SESSION`. (A no-restart "paste `!export BW_SESSION=…` into the prompt" path *seems* like it should work but doesn't: Claude Code's Bash tool spawns a fresh subprocess per call, so an export from the prompt's shell isn't visible to subsequent tool calls.)
- **`bw_ensure_ready` says "not logged in"** — same recovery, but `bw login &&` before the export.
- **`bw get item` returns nothing** — the item name is wrong. Use `bw list items --search boundless-ops-` to see what's actually there. Names are case-sensitive.
- **A custom field is missing** — Bitwarden lets you add fields via the web/desktop UI under "Custom fields"; pick **Text** for `db_url`, `chain`, `environment`, and **Hidden** for any secret you add.
- **Stale data after a rotation** — run `bw sync` to fetch the latest vault contents from the server.

## Known issues

### bw CLI 2026.3.0 / 2026.4.1 break non-interactive use

Upstream regression: `bw unlock` returns a session token but the vault state never persists as unlocked. Every subsequent `bw status` / `bw list` / `bw get` ignores `BW_SESSION` and re-prompts for the master password, breaking every automation (including these helpers and the migration script).

Tracking: [bitwarden/clients#20703](https://github.com/bitwarden/clients/issues/20703).

**Workaround:** pin to `2026.2.0` as shown in [One-time setup](#one-time-setup). Once a fixed release ships (anything > `2026.4.1` with the regression closed), `npm install -g @bitwarden/cli@<new-version>`.

### `bw get item <name>` does fuzzy substring matching

`bw get item boundless-ops-indexer-prod_base` matches both `prod_base` and `prod_base_sepolia`, returning a "More than one result was found" error. `bw-credentials.sh` works around this with `bw_get_by_name`, which does `bw list items --search` followed by a jq filter on the exact name. If you write any custom bw lookups, use the same pattern — never `bw get item <name>` directly.
