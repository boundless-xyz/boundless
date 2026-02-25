Create/update a pull request for the current changes.

1. Look at the staged, unstaged, and untracked changes with `git diff`
2. If no changes to examples/ directory, run `just check-main`. Else run `just check`.
   1. If fails, propose fixes and wait for approval
3. If there are untracked files, do not add them to the commit, but warn the user
4. Write a clear commit message based on what changed (use commit message style below)
5. Commit and push to the current branch
6. Check if an existing PR exists. Ignore warnings about GraphQL: Projects (classic) being deprecated.
   1. If not, use `gh pr create --web --title <title> --body <body>` to open the pre-populated create PR page with title/description (use PR Title/Description Guidelines below)
   2. Else update the description with the new changes.
7. Do not submit the PR. Just return the PR creation page URL when done

# Commit message style

- Simple, one sentence
- Do not include a `--trailer` when doing git commit

# PR Title/Description Guidelines

- Do not include made with Cursor or similar line
- Title/description should describe all changes across all commits, i.e. all the changes vs origin/main

## Writing Style — Sound Human, Not AI-Generated

Write like a developer talking to their team, not like a language model generating documentation.

### Structure and tone

- **Be direct and concise** — skip filler phrases like "This PR introduces...", "This change aims to...", "In order to facilitate..."
- **Lead with what changed and why** — not with a preamble about the problem space
- **Keep bullet points short** — 1-2 sentences max per bullet. No sub-bullets unless truly needed
- **Skip the obvious** — don't describe what's clear from the diff (e.g. "Updated imports" or "Added new file X.rs")
- **No sign-off or summary section** — just end when you're done
- **Match the tone of a brief Slack message**, not a design doc

### Banned words and phrases (AI vocabulary tells)

These words appear at dramatically higher rates in AI-generated text and are dead giveaways. Never use them:

- **Puffery:** "pivotal", "crucial role", "cornerstone", "testament to", "represents a significant", "broader landscape"
- **Fancy verbs:** "leverage" (say "use"), "facilitate" (say "help"), "showcase" (say "show"), "navigate" (say "handle"), "underscore", "foster", "bolster", "spearhead", "harness", "streamline", "delve"
- **Filler adjectives:** "robust", "comprehensive", "multifaceted", "nuanced", "holistic", "intricate", "seamless"
- **Hedging:** "It's worth noting...", "It should be noted...", "It's important to mention...", "One cannot overstate..."
- **False depth:** "Despite [positive], [subject] faces challenges...", "While [X], it's important to note [Y]..."

### Plain language replacements

- "addresses" → "fixes"
- "introduces" → "adds"
- "has been updated to" → "now does"
- "serves as a" → "is"
- "leverages" → "uses"
- "facilitates" → "helps" or "lets"
- "utilizing" → "using"
- "prior to" → "before"

### Other AI tells to avoid

- **Rule of three:** don't always list exactly 3 things. Sometimes 2 is enough, sometimes 4 is better
- **Synonym cycling:** don't avoid repeating a word by using increasingly weird synonyms. Just say "the function" twice instead of "the function" then "the aforementioned utility"
- **Formulaic structure:** don't make every bullet follow the same grammatical pattern
- **Excessive formatting:** don't bold every other phrase or use bullets where prose works fine

### Examples of AI-sounding vs human-sounding

❌ "This PR introduces a new indexing pipeline that aims to address the current limitations in our backfill process."
✅ "Adds a redrive lambda for re-processing failed indexer events."

❌ "The following changes have been implemented to improve the overall developer experience:"
✅ "Cleans up the CLI skill install flow — fewer prompts, better error messages."

❌ "This comprehensive update leverages a robust new architecture to facilitate seamless skill management."
✅ "Rewrites skill install to use symlinks instead of copying files."

## Template

```
<1-2 sentence overview: what changed and why>

Changes
* <specific notable change>
* <specific notable change>
* ...
```
