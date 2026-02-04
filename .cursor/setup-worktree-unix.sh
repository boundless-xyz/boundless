#!/bin/bash
set -e

# Copy all ignored files from the root worktree to the current worktree
# This ensures that ignored files (like .env files, build artifacts, etc.)
# are available in each worktree

if [ -z "$ROOT_WORKTREE_PATH" ]; then
  echo "Error: ROOT_WORKTREE_PATH environment variable is not set"
  exit 1
fi

# List ignored files from the root worktree and copy them to current worktree
(cd "$ROOT_WORKTREE_PATH" && git ls-files --ignored --exclude-standard -o --directory) | while read file; do
  if [ -e "$ROOT_WORKTREE_PATH/$file" ]; then
    # Create parent directories if they don't exist
    mkdir -p "$(dirname "$file")" 2>/dev/null || true
    
    # Copy the file or directory, preserving structure
    cp -r "$ROOT_WORKTREE_PATH/$file" "$file" 2>/dev/null || true
  fi
done
