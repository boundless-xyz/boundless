#!/bin/bash
set -e

# Copy ignored files from the root worktree to the current worktree
# Excludes build artifacts (target/, node_modules/, out/, cache/) which 
# should be rebuilt in the worktree (to avoid copying large stale artifacts)

if [ -z "$ROOT_WORKTREE_PATH" ]; then
  echo "Error: ROOT_WORKTREE_PATH environment variable is not set"
  exit 1
fi

# Patterns to exclude (build artifacts are not copied)
# These match both top-level and nested paths
# Also excludes directories that contain ONLY build artifacts
EXCLUDE_PATTERN="(target/|node_modules/|/out/|^out/|/cache/|^cache/|/build/|/bin/|build-info|\.DS_Store|cache_market|^documentation/|crates/guest/povw/)"

# List ignored files, exclude build artifacts, then copy
(cd "$ROOT_WORKTREE_PATH" && git ls-files --ignored --exclude-standard -o --directory) | \
  grep -Ev "$EXCLUDE_PATTERN" | \
  while read file; do
    if [ -e "$ROOT_WORKTREE_PATH/$file" ]; then
      # Create parent directories if they don't exist
      mkdir -p "$(dirname "$file")" 2>/dev/null || true
      
      # Copy the file or directory, preserving structure
      cp -r "$ROOT_WORKTREE_PATH/$file" "$file" 2>/dev/null || true
    fi
  done

echo "Worktree setup complete. Run 'cargo build' and 'forge build' to rebuild artifacts."
