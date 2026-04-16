#!/bin/bash
set -e

# Copy ignored files from the root worktree to the current worktree.
# Excludes build artifacts (target/, node_modules/, out/, cache/) which
# should be rebuilt in the worktree to avoid copying large stale artifacts.

if [ -z "$CONDUCTOR_ROOT_PATH" ]; then
  echo "Error: CONDUCTOR_ROOT_PATH environment variable is not set"
  exit 1
fi

EXCLUDE_PATTERN="(target/|node_modules/|/out/|^out/|/cache/|^cache/|/build/|/bin/|build-info|\.DS_Store|cache_market|^documentation/|crates/guest/povw/)"

(cd "$CONDUCTOR_ROOT_PATH" && git ls-files --ignored --exclude-standard -o --directory) | \
  grep -Ev "$EXCLUDE_PATTERN" | \
  while read file; do
    if [ -e "$CONDUCTOR_ROOT_PATH/$file" ]; then
      mkdir -p "$(dirname "$file")" 2>/dev/null || true
      cp -r "$CONDUCTOR_ROOT_PATH/$file" "$file" 2>/dev/null || true
    fi
  done

echo "Worktree setup complete. Run 'cargo build' and 'forge build' to rebuild artifacts."
