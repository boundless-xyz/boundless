// Copyright 2026 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Install Boundless skill files for AI coding tools.

use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use inquire::Select;

use crate::config::GlobalConfig;

// Skill file contents embedded at compile time.
const SKILL_MD: &str = include_str!("../../../skill/SKILL.md");
const ARCHITECTURE_MD: &str = include_str!("../../../skill/references/architecture.md");
const CLI_REFERENCE_MD: &str = include_str!("../../../skill/references/cli-reference.md");
const CONVENTIONS_MD: &str = include_str!("../../../skill/references/conventions.md");

/// Supported AI tool formats and their install paths.
#[derive(Clone, Debug)]
struct ToolFormat {
    name: &'static str,
    description: &'static str,
    local_dir: &'static str,
    global_dir: &'static str,
}

impl fmt::Display for ToolFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.name, self.description)
    }
}

const TOOL_FORMATS: &[ToolFormat] = &[
    ToolFormat {
        name: "Claude Code",
        description: "Claude Code skill format (.claude/skills/boundless)",
        local_dir: ".claude/skills/boundless",
        global_dir: ".claude/skills/boundless",
    },
    ToolFormat {
        name: "OpenCode",
        description: "OpenCode AI skill format (.opencode/skills/boundless)",
        local_dir: ".opencode/skills/boundless",
        global_dir: ".opencode/skills/boundless",
    },
    ToolFormat {
        name: "Codex",
        description: "Codex skill format (.codex/skills/boundless)",
        local_dir: ".codex/skills/boundless",
        global_dir: ".codex/skills/boundless",
    },
    ToolFormat {
        name: "GitHub Copilot",
        description: "GitHub Copilot instructions (.github/skills/boundless)",
        local_dir: ".github/skills/boundless",
        global_dir: ".copilot/skills/boundless",
    },
    ToolFormat {
        name: "Cursor",
        description: "Cursor AI skill format (.cursor/skills/boundless)",
        local_dir: ".cursor/skills/boundless",
        global_dir: ".cursor/skills/boundless",
    },
    ToolFormat {
        name: "Windsurf",
        description: "Windsurf skill format (.windsurf/skills/boundless)",
        local_dir: ".windsurf/skills/boundless",
        global_dir: ".windsurf/skills/boundless",
    },
];

/// Install Boundless skill files for AI coding tools
#[derive(Args, Clone, Debug)]
pub struct SkillInstall {
    /// Install to the home directory instead of the current directory
    #[arg(long, short = 'g')]
    global: bool,

    /// Custom install path (overrides default location)
    #[arg(long, short = 'p')]
    path: Option<PathBuf>,
}

impl SkillInstall {
    /// Run the skill install command.
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let install_dir = if let Some(ref path) = self.path {
            path.clone()
        } else {
            let format = Select::new(
                "Select AI tool format:",
                TOOL_FORMATS.to_vec(),
            )
            .prompt()
            .context("Failed to get tool format selection")?;

            if self.global {
                let home = dirs::home_dir().context("Failed to get home directory")?;
                home.join(format.global_dir)
            } else {
                let cwd =
                    std::env::current_dir().context("Failed to get current directory")?;
                cwd.join(format.local_dir)
            }
        };

        // Create directory structure
        let refs_dir = install_dir.join("references");
        std::fs::create_dir_all(&refs_dir)
            .with_context(|| format!("Failed to create directory: {}", refs_dir.display()))?;

        // Write skill files
        let files: &[(&str, &str)] = &[
            ("SKILL.md", SKILL_MD),
            ("references/architecture.md", ARCHITECTURE_MD),
            ("references/cli-reference.md", CLI_REFERENCE_MD),
            ("references/conventions.md", CONVENTIONS_MD),
        ];

        for (name, content) in files {
            let file_path = install_dir.join(name);
            std::fs::write(&file_path, content)
                .with_context(|| format!("Failed to write file: {}", file_path.display()))?;
        }

        // Print success message
        println!();
        println!("{} Boundless skill installed successfully!", "✓".green().bold());
        println!();

        // Show relative path if possible, otherwise absolute
        let display_path = if let Ok(cwd) = std::env::current_dir() {
            install_dir
                .strip_prefix(&cwd)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| install_dir.display().to_string())
        } else {
            install_dir.display().to_string()
        };

        println!("  {}: {}", "Location".bold(), display_path);
        println!();
        println!("  {}:", "Files installed".bold());
        for (name, _) in files {
            println!("    {} {}", "•".dimmed(), name);
        }
        println!();

        Ok(())
    }
}
