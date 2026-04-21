#!/bin/bash
set -euo pipefail

# Copy ignored files from the source tree to the new worktree while skipping
# build artifacts that should be rebuilt in the worktree.

if [ -z "${CODEX_SOURCE_TREE_PATH:-}" ]; then
  echo "Error: CODEX_SOURCE_TREE_PATH environment variable is not set"
  exit 1
fi

if [ -z "${CODEX_WORKTREE_PATH:-}" ]; then
  echo "Error: CODEX_WORKTREE_PATH environment variable is not set"
  exit 1
fi

SOURCE_TREE_PATH="${CODEX_SOURCE_TREE_PATH%/}"
WORKTREE_PATH="${CODEX_WORKTREE_PATH%/}"

EXCLUDE_PATTERN="(target/|node_modules/|/out/|^out/|/cache/|^cache/|/build/|/bin/|build-info|\\.DS_Store|cache_market|^documentation/|crates/guest/povw/)"

while IFS= read -r file; do
  [ -n "$file" ] || continue

  source_path="$SOURCE_TREE_PATH/$file"
  worktree_path="$WORKTREE_PATH/$file"

  if [ -e "$source_path" ]; then
    mkdir -p "$(dirname "$worktree_path")" 2>/dev/null || true
    cp -R "$source_path" "$worktree_path" 2>/dev/null || true
  fi
done < <(
  cd "$SOURCE_TREE_PATH"
  git ls-files --ignored --exclude-standard -o --directory | grep -Ev "$EXCLUDE_PATTERN" || true
)

echo "Worktree setup complete. Run 'cargo build' and 'forge build' to rebuild artifacts."
