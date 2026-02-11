---
name: contributing-skills
description: How to create, test, and ship new Boundless CLI skills. Triggers include "create a skill", "write a skill", "new skill", "contribute a skill", "skill development", "add a skill".
---

# Contributing Skills

This skill teaches you how to create, test, and ship new skills for the Boundless CLI.

## What Is a Skill?

A skill is a set of markdown files (and optional scripts/examples) that get installed into an AI coding tool's skill directory (`.claude/skills/`, `.cursor/skills/`, etc.). Skills give AI agents context about Boundless — how to use the CLI, build on the protocol, troubleshoot issues, etc.

Skills are embedded into the `boundless` binary at compile time via `include_str!()`, so end users get them with a single `boundless skill install` command — no network requests needed.

## Skill Directory Structure

All skill source files live under `crates/boundless-cli/skill/`:

```
crates/boundless-cli/skill/
├── boundless-overview/
│   ├── SKILL.md                          # Entry point (required)
│   └── references/
│       ├── architecture.md
│       ├── cli-reference.md
│       └── conventions.md
├── first-request/
│   ├── SKILL.md
│   ├── scripts/
│   │   ├── check-prerequisites.sh
│   │   └── discover-programs.sh
│   ├── examples/
│   │   └── request.yaml
│   └── references/
│       ├── cli-reference.md
│       ├── guest-program-explainer.md
│       └── troubleshooting.md
└── contributing-skills/                   # This skill!
    ├── SKILL.md
    └── references/
        └── registration-guide.md
```

### Required: `SKILL.md`

Every skill must have a `SKILL.md` at its root. This is the entry point that AI tools read first.

**Frontmatter** (required):

```yaml
---
name: my-skill-name
description: One-line description. Triggers include "phrase 1", "phrase 2", "phrase 3".
---
```

- `name` — kebab-case, matches the directory name
- `description` — Describes what the skill does, plus natural language trigger phrases that help AI tools know when to activate this skill

**Body** — Write this for an AI agent audience. Be explicit, step-by-step, and include exact commands. The agent will follow these instructions when a user's request matches the trigger phrases.

### Optional Subdirectories

| Directory | Purpose |
|-----------|---------|
| `references/` | Deep-dive docs, API references, architecture details |
| `scripts/` | Shell scripts the agent can execute (mark executable) |
| `examples/` | Example configs, YAML files, code snippets |

## Development Workflow

### Step 1: Create the Skill Files

```bash
mkdir -p crates/boundless-cli/skill/my-new-skill/references
```

Write your `SKILL.md` and any supporting files.

### Step 2: Test with `boundless skill dev`

This is the fast iteration loop. Instead of recompiling the CLI, symlink the source files directly into your AI tool's skill directory:

```bash
# Symlink all skills (including your new one) into Claude Code's skill dir
cargo run --bin boundless -- skill dev --path .claude/skills

# Or symlink just your skill
cargo run --bin boundless -- skill dev my-new-skill --path .claude/skills

# When you want a specific tool format with interactive selection
cargo run --bin boundless -- skill dev
```

Now edit the source files in `crates/boundless-cli/skill/my-new-skill/` and the AI tool sees changes instantly — no recompile needed.

To remove the symlinks when done:

```bash
cargo run --bin boundless -- skill dev --clean --path .claude/skills
```

### Step 3: Test the Skill with an AI Agent

With the symlinks in place, open your AI tool and try triggering the skill:

1. Start a new conversation
2. Ask something that matches your trigger phrases
3. Verify the agent finds and follows your skill instructions
4. Iterate on the markdown — changes are live immediately

**Tips for testing:**
- Test with multiple phrasings to verify trigger coverage
- Verify any scripts are executable and work from the skill directory
- Check that relative file references (`references/foo.md`, `scripts/bar.sh`) resolve correctly
- Test the full workflow end-to-end, not just individual steps

### Step 4: Register the Skill in `install.rs`

Once the skill content is solid, register it in `crates/boundless-cli/src/commands/skill/install.rs` so it gets embedded in the binary.

See `references/registration-guide.md` for the exact code changes needed.

### Step 5: Add Integration Tests

Add a test in `crates/boundless-cli/tests/skill/install.rs` that verifies your skill installs correctly:

```rust
/// Test that my-new-skill installs all files correctly.
#[test]
fn test_skill_install_my_new_skill_with_path() {
    let tmp_dir = TempDir::new().unwrap();
    let install_path = tmp_dir.path().join("test-my-skill");

    let mut cmd = cli_cmd().unwrap();

    cmd.args([
        "skill",
        "install",
        "my-new-skill",
        "--path",
        install_path.to_str().unwrap(),
    ])
    .env("NO_COLOR", "1")
    .assert()
    .success()
    .stdout(contains("installed successfully"));

    // Verify all files were created.
    let skill_dir = install_path.join("my-new-skill");
    assert!(skill_dir.join("SKILL.md").exists());
    // Add assertions for all your files...
}
```

Also update the `test_skill_install_all_with_path` and `test_skill_install_list` tests to include your new skill.

### Step 6: Run Tests

```bash
cargo test -p boundless-cli --test integration -- skill
```

### Step 7: Submit a PR

Follow the repo's standard PR process. The PR should include:
- New skill files in `crates/boundless-cli/skill/<name>/`
- Registration in `install.rs`
- Integration tests
- Updated `test_skill_install_all_with_path` and `test_skill_install_list` tests

## Writing Good Skills

### Audience
Your reader is an AI coding agent (Claude, GPT, Copilot, etc.), not a human. Write instructions that a language model can follow precisely.

### Structure
- Start with a clear objective: what does the user want to accomplish?
- Break the workflow into numbered phases
- Include exact commands — don't leave things ambiguous
- Reference supporting files with relative paths (`references/foo.md`, `scripts/bar.sh`)
- Add a quick-reference table of key commands at the top

### Trigger Phrases
Include diverse trigger phrases in the `description` frontmatter. Think about how a user would ask for this — informal, formal, partial, synonym-based.

### Error Handling
Document what can go wrong and how to fix it. AI agents are good at troubleshooting if you give them the error patterns and solutions.

### Keep It Focused
One skill = one workflow or topic area. Don't try to cover everything in one skill. Users should be able to install just the skills they need.

## Reference Files

- [Registration Guide](references/registration-guide.md) — Exact code changes to register a new skill in `install.rs`
