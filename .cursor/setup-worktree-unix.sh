#!/bin/bash
set -e

# Copy all ignored files from the root worktree to the current worktree
# This ensures that ignored files (like .env files, build artifacts, etc.)
# are available in each worktree

git ls-files --ignored --exclude-standard -o --directory | while read file; do
  if [ -e "$ROOT_WORKTREE_PATH/$file" ]; then
    # Create parent directories if they don't exist
    mkdir -p "$(dirname "$file")" 2>/dev/null || true
    
    # Copy the file or directory, preserving structure
    cp -r "$ROOT_WORKTREE_PATH/$file" "$file" 2>/dev/null || true
  fi
done
