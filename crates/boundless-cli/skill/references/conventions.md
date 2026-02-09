# Boundless Development Conventions

## CLI Command Implementation Pattern

Each CLI command follows this pattern:

```rust
use anyhow::Result;
use clap::Args;
use crate::config::GlobalConfig;

/// Description of what the command does
#[derive(Args, Clone, Debug)]
pub struct MyCommand {
    /// Help text for this flag
    #[arg(long, env = "MY_ENV_VAR")]
    my_flag: Option<String>,
}

impl MyCommand {
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // Load config from file, merge with CLI args and env vars
        let config = self.load_and_validate(global_config)?;

        // Command implementation
        Ok(())
    }
}
```

### Command Module Structure

Commands are organized in module directories under `src/commands/`:
```
src/commands/
├── mod.rs              # Declares all command modules
├── requestor/
│   ├── mod.rs          # RequestorCommands enum with #[derive(Subcommand)]
│   ├── balance.rs      # Individual command implementations
│   ├── deposit.rs
│   └── ...
├── prover/
│   ├── mod.rs          # ProverCommands enum
│   └── ...
└── rewards/
    ├── mod.rs          # RewardsCommands enum
    └── ...
```

Each module's `mod.rs` defines a `#[derive(Subcommand)]` enum and a `run()` method that dispatches to individual commands.

## Config Loading

Use `load_and_validate()` to merge configuration from multiple sources:
- CLI arguments (highest priority)
- Environment variables
- Config file (`~/.boundless/config.toml` + `secrets.toml`)
- Network defaults (lowest priority)

The `GlobalConfig` struct is passed through from the top-level CLI parser and contains global options like `--tx-timeout` and `--log-level`.

## Display Output

Use `DisplayManager` from `crate::display` for consistent output:

```rust
use crate::display::DisplayManager;

let display = DisplayManager::with_network("base-mainnet");
display.header("My Command");
display.item("Label", value);
display.success("Operation completed");
display.warning("Something to note");
```

For simple colored output without `DisplayManager`, use the `colored` crate:
```rust
use colored::Colorize;
println!("{} Done!", "✓".green().bold());
```

## Error Handling

- Use `anyhow::Result` for all fallible functions
- Use `.context("description")` to add context to errors
- Use `bail!("message")` for early returns with errors
- Avoid `unwrap()` in library code; prefer `?` operator

## Interactive Prompts

Use `inquire` for interactive user input:
```rust
use inquire::{Select, Confirm, Text};

let choice = Select::new("Pick one:", vec!["A", "B", "C"])
    .prompt()
    .context("Failed to get selection")?;
```

## License Headers

All source files must include the Apache 2.0 license header:
```rust
// Copyright 2026 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// ...
```

## Testing

- Integration tests use `assert_cmd` with `cargo_bin("boundless")`
- Use `predicates` for assertion matching
- Use `tempfile::TempDir` for temporary directories
- Set `NO_COLOR=1` in tests to avoid ANSI escape codes
- Tests live in `tests/` directory, organized by module
