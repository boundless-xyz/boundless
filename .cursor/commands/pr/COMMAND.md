Create/update a pull request for the current changes.

1. Look at the staged, unstaged, and untracked changes with `git diff`
2. If no changes to examples/ directory, run `just check-main`. Else run `just check`.
   1. If fails, propose fixes and wait for approval
3. If there are untracked files, do not add them to the commit, but warn the user
4. Write a clear commit message based on what changed (use commit message style below)
5. Commit and push to the current branch
6. Check if an existing PR exists. Ignore warnings about GraphQL: Projects (classic) being deprecated.
   1. If not, construct the GitHub PR creation URL and open it in the browser so the user can review before submitting:
      ```
      REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner)
      BRANCH=$(git branch --show-current)
      BASE=$(gh repo view --json defaultBranchRef -q .defaultBranchRef.name)
      TITLE=$(python3 -c "import urllib.parse; print(urllib.parse.quote('<title>'))")
      BODY=$(python3 -c "import urllib.parse; print(urllib.parse.quote('<body>'))")
      URL="https://github.com/${REPO}/compare/${BASE}...${BRANCH}?expand=1&title=${TITLE}&body=${BODY}"
      open "$URL"
      ```
      Use the PR Title/Description Guidelines below for title and body.
      IMPORTANT: Do NOT use `gh pr create` (without --web it submits directly, with --web it prints nothing in non-TTY shells).
   2. Else update the description with `gh pr edit`.
7. Return the PR URL when done

# Commit message style

- Simple, one sentence
- Do not include a `--trailer` when doing git commit

# PR Title/Description Guidelines

- Do not include made with Cursor or similar line
- Title/description shoudl describe all changes across all commits, i.e. all the changes vs origin/main

Template:

```
<high level overview of changes, and motivations for the changes>

Changes
* <1-2 sentences for specific notable changes>
* ...
```
