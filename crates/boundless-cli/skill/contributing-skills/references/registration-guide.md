# Registering a New Skill in `install.rs`

After creating your skill files, you need to register the skill in the install command so it gets embedded in the binary.

## File: `crates/boundless-cli/src/commands/skill/install.rs`

### 1. Add the Skill to the `SKILLS` Array

Find the `const SKILLS: &[Skill]` array and add a new entry. Each file is included at compile time via `include_str!()`:

```rust
const SKILLS: &[Skill] = &[
    // ... existing skills ...
    Skill {
        name: "my-new-skill",
        description: "What this skill does",
        files: &[
            (
                "SKILL.md",
                include_str!("../../../skill/my-new-skill/SKILL.md"),
            ),
            (
                "references/some-reference.md",
                include_str!("../../../skill/my-new-skill/references/some-reference.md"),
            ),
            (
                "scripts/my-script.sh",
                include_str!("../../../skill/my-new-skill/scripts/my-script.sh"),
            ),
        ],
    },
];
```

**Key details:**

- `name` — Must match the directory name under `crates/boundless-cli/skill/`
- `description` — Short description shown in `boundless skill install --list`
- `files` — Each tuple is `(relative_path, content)`. The `relative_path` is where the file is installed relative to the skill's install directory.
- The `include_str!()` path is relative to the `install.rs` source file: `../../../skill/<name>/<path>`

### 2. Verify the Include Paths

The `include_str!()` paths are relative to `crates/boundless-cli/src/commands/skill/install.rs`. To reach the skill directory:

```
install.rs location:  crates/boundless-cli/src/commands/skill/install.rs
skill directory:      crates/boundless-cli/skill/
relative path:        ../../../skill/
```

So `include_str!("../../../skill/my-new-skill/SKILL.md")` resolves correctly.

### 3. File Permissions for Scripts

Note that `include_str!()` embeds content as text. When installed, scripts won't automatically be executable. If your skill includes shell scripts that the agent should run, mention in your `SKILL.md` that the agent should either:
- Run them with `bash scripts/my-script.sh` (explicit interpreter), or
- `chmod +x` them first

### 4. Compile and Test

```bash
# Verify it compiles (include_str will fail if a path is wrong)
cargo check -p boundless-cli

# Run the skill tests
cargo test -p boundless-cli --test integration -- skill
```

If any `include_str!()` path is wrong, you'll get a clear compile error pointing to the bad path.

## Complete Example

Here's the full diff for adding a hypothetical "deploy-verifier" skill with 3 files:

```rust
Skill {
    name: "deploy-verifier",
    description: "Deploy and configure an on-chain verifier for your ZK application",
    files: &[
        (
            "SKILL.md",
            include_str!("../../../skill/deploy-verifier/SKILL.md"),
        ),
        (
            "references/contract-api.md",
            include_str!("../../../skill/deploy-verifier/references/contract-api.md"),
        ),
        (
            "examples/deploy-config.toml",
            include_str!("../../../skill/deploy-verifier/examples/deploy-config.toml"),
        ),
    ],
},
```
