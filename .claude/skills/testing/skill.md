---
name: testing
description: Run this repo's Rust verification before claiming work is done—cargo fmt, just check-clippy-main (CI-equivalent clippy), and tests from the justfile (cargo test, forge, Postgres-backed crates). Use after substantive edits, when the user asks to run tests, validate, or match CI, when investigating clippy or test failures, or any time correctness is uncertain.
---

# Verify Rust and Solidity changes

## When to use

- You changed Rust (`crates/`, `*.rs`) behavior or APIs.
- The user asked to verify, run tests, match CI, or fix clippy.
- You are about to summarize work as complete—check first unless the edit was trivial.

Do not wait to be asked for non-trivial edits. Narrow commands to what you touched (e.g. `cargo test -p broker --lib -- filter`) but follow `justfile` recipes and env vars.

**Do not edit this skill directory (including `SKILL.md` and `gotchas.md`) or the repo `justfile` unless the user explicitly confirms.** Suggest changes in your reply instead.

## What to run

1. `cargo fmt --all` — format first, always.
2. `just check-clippy-main` — clippy with `-Dwarnings`. This is what CI runs. Do not skip it.
3. Tests — pick based on what changed. Read the `justfile` for the canonical recipes. Common ones:
   - `just test-broker` — broker + boundless-market config tests
   - `just test-cargo` — full Rust test suite
   - `just test-foundry` — Solidity tests
   - Or run targeted: `RISC0_DEV_MODE=1 cargo test -p <crate> --lib -- <filter>`

## Gotchas

Read `gotchas.md` in this skill directory. It lists environment and build pitfalls that cause false failures. If a test fails unexpectedly, check the gotchas before debugging.
