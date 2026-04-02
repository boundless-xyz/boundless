---
name: release-notes
description: >-
  Internal — for Boundless team members only. Generate release notes for a new Boundless version. Fetches PR details,
  explores code changes, and produces prover-facing documentation with config
  examples, migration guides, and worked examples. Use when the user wants to
  write release notes, changelog entries, or upgrade guides.
---

# Release Notes Generator

Generate release notes for a new Boundless release. The audience for broker changes is **prover operators** -- people running the broker who need to understand what changed, how to configure it, and how to migrate.

## Process

### Step 1: Gather inputs

Ask the user for:

1. **Version number** for this release
2. **Previous release tag** to compare against (e.g., `v1.2.2`)
3. **Key PRs or features** to highlight -- the user will tell you which PRs or topics matter. Don't try to cover every commit.
4. **Current market context** if relevant (e.g., current pricing in USD/Bcycle, token prices) for realistic examples

### Step 2: Research

For each feature the user wants covered:

1. Fetch PR details with `gh pr view <number> --repo boundless-xyz/boundless --json title,body`
2. Read the actual code to understand how the feature works -- don't rely solely on PR descriptions
3. Read config structs, default values, and broker.toml template to get accurate config examples
4. Check `compose.yml` for env var support and `justfile` for how provers actually run things (`just prover up`)

Use subagents to research multiple PRs in parallel.

### Step 3: Write the notes

Follow these principles:

#### Structure

- Start with a brief overview paragraph summarizing all highlighted features in one sentence
- Include a full changelog link: `[vOLD...vNEW](https://github.com/boundless-xyz/boundless/compare/vOLD...vNEW)`
- Each feature gets its own section with: what changed (1-3 sentences on the change, new paragraph, then 1-3 sentences on why it matters), how to use it (how to enable with examples)
- End with an "Upgrade Notes" section summarizing action items

#### Writing style

- Write for prover operators, not developers. Focus on config changes, CLI flags, and operational impact.
- Lead with what the user needs to do, not how the code works internally
- Be direct and concise. No filler, no marketing language.
- Use "the broker" not "we" or "the system"
- Don't use emojis

#### Config examples

- Show the recommended/minimal config first, not the full config with all options
- Use realistic values based on current market rates (ask the user if unsure)
- For broker.toml changes, always show the TOML syntax exactly as it would appear in the file. Show before + after if relevant.
- For CLI flags, show how to pass them via `BROKER_EXTRA_ARGS` in `.env` (since most provers use `just prover up`) AND as direct CLI flags
- For env vars, show the `export` syntax

#### Migration guides

- Show a clear before/after comparison with labeled TOML blocks
- Call out which fields are removed, renamed, or have new defaults
- Always note backward compatibility -- existing configs should work without changes unless there's a breaking change

#### Worked examples

- Use real order data where possible. Ask the user for a reference order from the explorer if they don't provide one.
- Convert all comparisons to USD when demonstrating USD-based features
- Show the math step by step so operators can verify their own config

#### Features to watch for

When researching broker changes, pay attention to:

- **Pricing**: `min_mcycle_price`, collateral pricing, pricing overrides in `crates/boundless-market/src/prover_utils/config.rs`
- **Config**: broker.toml template at `broker-template.toml`, MarketConfig struct
- **RPC/monitoring**: chain monitors, gas estimation in `crates/broker/src/`
- **CLI flags**: Args struct in `crates/broker/src/lib.rs`, check for `BROKER_EXTRA_ARGS` support in `compose.yml`
- **Telemetry**: `crates/broker/src/telemetry.rs`
- **Hot-reload**: note when config changes take effect without restart

### Step 4: Iterate

The user will review and give feedback. Common adjustments:

- Reordering sections to lead with the most impactful change
- Adjusting example values to match current market conditions
- Adding or removing detail from specific sections
- Changing the framing of a feature (e.g., "show the simplification, not the mechanism")

## References

Use v1.4.0 release from https://github.com/boundless-xyz/boundless/releases as a reference for tone, structure, and level of detail.
