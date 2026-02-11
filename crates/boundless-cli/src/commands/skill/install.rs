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

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use inquire::Select;

use crate::config::GlobalConfig;

/// A skill definition with embedded file contents.
struct Skill {
    /// Unique name for this skill (used as directory name).
    name: &'static str,
    /// Short description of what this skill provides.
    description: &'static str,
    /// Files to install: (relative_path, content).
    files: &'static [(&'static str, &'static str)],
}

impl fmt::Display for Skill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.name, self.description)
    }
}

/// All available skills embedded at compile time.
const SKILLS: &[Skill] = &[
    Skill {
        name: "boundless-overview",
        description: "Boundless CLI overview, architecture, and conventions",
        files: &[
            (
                "SKILL.md",
                include_str!("../../../skill/boundless-overview/SKILL.md"),
            ),
            (
                "references/architecture.md",
                include_str!("../../../skill/boundless-overview/references/architecture.md"),
            ),
            (
                "references/cli-reference.md",
                include_str!("../../../skill/boundless-overview/references/cli-reference.md"),
            ),
            (
                "references/conventions.md",
                include_str!("../../../skill/boundless-overview/references/conventions.md"),
            ),
        ],
    },
    Skill {
        name: "first-request",
        description: "Guided walkthrough: submit your first ZK proof on Boundless",
        files: &[
            (
                "SKILL.md",
                include_str!("../../../skill/first-request/SKILL.md"),
            ),
            (
                "scripts/check-prerequisites.sh",
                include_str!("../../../skill/first-request/scripts/check-prerequisites.sh"),
            ),
            (
                "scripts/discover-programs.sh",
                include_str!("../../../skill/first-request/scripts/discover-programs.sh"),
            ),
            (
                "scripts/build-request-yaml.sh",
                include_str!("../../../skill/first-request/scripts/build-request-yaml.sh"),
            ),
            (
                "examples/request.yaml",
                include_str!("../../../skill/first-request/examples/request.yaml"),
            ),
            (
                "references/cli-reference.md",
                include_str!("../../../skill/first-request/references/cli-reference.md"),
            ),
            (
                "references/guest-program-explainer.md",
                include_str!("../../../skill/first-request/references/guest-program-explainer.md"),
            ),
            (
                "references/troubleshooting.md",
                include_str!("../../../skill/first-request/references/troubleshooting.md"),
            ),
        ],
    },
    Skill {
        name: "contributing-skills",
        description: "How to create, test, and ship new Boundless CLI skills",
        files: &[
            (
                "SKILL.md",
                include_str!("../../../skill/contributing-skills/SKILL.md"),
            ),
            (
                "references/registration-guide.md",
                include_str!("../../../skill/contributing-skills/references/registration-guide.md"),
            ),
        ],
    },
];

/// Supported AI tool formats and their install paths.
#[derive(Clone, Debug)]
pub struct ToolFormat {
    pub name: &'static str,
    pub description: &'static str,
    pub local_dir: &'static str,
    pub global_dir: &'static str,
}

impl fmt::Display for ToolFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.name, self.description)
    }
}

pub const TOOL_FORMATS: &[ToolFormat] = &[
    ToolFormat {
        name: "Claude Code",
        description: "Claude Code skill format (.claude/skills)",
        local_dir: ".claude/skills",
        global_dir: ".claude/skills",
    },
    ToolFormat {
        name: "OpenCode",
        description: "OpenCode AI skill format (.opencode/skills)",
        local_dir: ".opencode/skills",
        global_dir: ".opencode/skills",
    },
    ToolFormat {
        name: "Codex",
        description: "Codex skill format (.codex/skills)",
        local_dir: ".codex/skills",
        global_dir: ".codex/skills",
    },
    ToolFormat {
        name: "Pi",
        description: "Pi coding agent skill format (.pi/skills)",
        local_dir: ".pi/skills",
        global_dir: ".pi/agent/skills",
    },
    ToolFormat {
        name: "GitHub Copilot",
        description: "GitHub Copilot instructions (.github/skills)",
        local_dir: ".github/skills",
        global_dir: ".copilot/skills",
    },
    ToolFormat {
        name: "Cursor",
        description: "Cursor AI skill format (.cursor/skills)",
        local_dir: ".cursor/skills",
        global_dir: ".cursor/skills",
    },
    ToolFormat {
        name: "Windsurf",
        description: "Windsurf skill format (.windsurf/skills)",
        local_dir: ".windsurf/skills",
        global_dir: ".windsurf/skills",
    },
];

/// Install Boundless skill files for AI coding tools
#[derive(Args, Clone, Debug)]
pub struct SkillInstall {
    /// Name of the skill to install (e.g., boundless-overview, first-request)
    #[arg()]
    skill_name: Option<String>,

    /// Install all available skills
    #[arg(long)]
    all: bool,

    /// List available skills without installing
    #[arg(long)]
    list: bool,

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
        // Handle --list: print available skills and return.
        if self.list {
            println!();
            println!("{}", "Available skills:".bold());
            println!();
            for skill in SKILLS {
                println!("  {} {}", skill.name.bold(), format!("- {}", skill.description).dimmed());
            }
            println!();
            return Ok(());
        }

        // Determine which skills to install.
        let skills_to_install: Vec<&Skill> = if self.all {
            SKILLS.iter().collect()
        } else if let Some(ref name) = self.skill_name {
            let skill = SKILLS.iter().find(|s| s.name == name.as_str());
            match skill {
                Some(s) => vec![s],
                None => {
                    let available: Vec<&str> = SKILLS.iter().map(|s| s.name).collect();
                    bail!(
                        "Unknown skill '{}'. Available skills: {}",
                        name,
                        available.join(", ")
                    );
                }
            }
        } else {
            // Interactive selection.
            let options: Vec<String> = SKILLS.iter().map(|s| s.to_string()).collect();
            let selection = Select::new("Select a skill to install:", options)
                .prompt()
                .context("Failed to get skill selection")?;
            // Find the skill by matching the display string.
            let skill = SKILLS
                .iter()
                .find(|s| s.to_string() == selection)
                .context("Failed to find selected skill")?;
            vec![skill]
        };

        // Resolve the base install directory (without skill name).
        let base_dir = if let Some(ref path) = self.path {
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

        // Install each skill.
        for skill in &skills_to_install {
            let install_dir = base_dir.join(skill.name);

            // Collect all subdirectories needed.
            for (rel_path, _) in skill.files {
                if let Some(parent) = std::path::Path::new(rel_path).parent() {
                    if !parent.as_os_str().is_empty() {
                        let dir = install_dir.join(parent);
                        std::fs::create_dir_all(&dir)
                            .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
                    }
                }
            }
            // Ensure the base install dir exists even if all files are at root level.
            std::fs::create_dir_all(&install_dir)
                .with_context(|| format!("Failed to create directory: {}", install_dir.display()))?;

            // Write all files.
            for (rel_path, content) in skill.files {
                let file_path = install_dir.join(rel_path);
                std::fs::write(&file_path, content)
                    .with_context(|| format!("Failed to write file: {}", file_path.display()))?;
            }

            // Print success message.
            println!();
            println!(
                "{} Skill '{}' installed successfully!",
                "✓".green().bold(),
                skill.name
            );
            println!();

            // Show relative path if possible, otherwise absolute.
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
            for (name, _) in skill.files {
                println!("    {} {}", "•".dimmed(), name);
            }
            println!();
        }

        Ok(())
    }
}
