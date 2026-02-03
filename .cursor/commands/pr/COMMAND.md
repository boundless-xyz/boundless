Create a pull request for the current changes.
1. Look at the staged, unstaged, and untracked changes with `git diff`
2. If there are untracked files, do not add them to the commit, but warn the user
3. Write a clear commit message based on what changed (use commit message style below)
4. Commit and push to the current branch
5. Use `gh pr create --web --title <title> --body <body>` to generate a pull request URL with title/description (use PR Title/Description Guidelines below)
6. Return the PR URL when done

# Commit message style
* Simple, one sentence
* Do not include co-author line

# PR Title/Description Guidelines
* Do not include made with Cursor or similar line
* Title/description shoudl describe all changes across all commits, i.e. all the changes vs origin/main

Template: 
```
<high level overview of changes, and motivations for the changes>

Changes
* <1-2 sentences for specific notable changes>
* ...
```
