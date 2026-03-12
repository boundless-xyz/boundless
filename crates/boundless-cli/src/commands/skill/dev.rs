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

//! Symlink skill source files for rapid development iteration.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use inquire::Select;

use crate::config::GlobalConfig;

use super::install::TOOL_FORMATS;

/// Symlink skill source files for live development (no recompile needed)
#[derive(Args, Clone, Debug)]
pub struct SkillDev {
    /// Name of the skill to link (default: all skills in the skill directory)
    #[arg()]
    skill_name: Option<String>,

    /// Install to the home directory instead of the current directory
    #[arg(long, short = 'g')]
    global: bool,

    /// Custom target path (overrides default location)
    #[arg(long, short = 'p')]
    path: Option<PathBuf>,

    /// Remove dev symlinks instead of creating them
    #[arg(long)]
    clean: bool,
}

/// Find the workspace root by walking up from the current directory looking for
/// the workspace Cargo.toml that contains `[workspace]`.
fn find_workspace_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir().context("Failed to get current directory")?;
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml)
                .with_context(|| format!("Failed to read {}", cargo_toml.display()))?;
            if content.contains("[workspace]") {
                return Ok(dir);
            }
        }
        if !dir.pop() {
            bail!(
                "Could not find workspace root (Cargo.toml with [workspace]). \
                 Run this command from within the Boundless repo."
            );
        }
    }
}

/// Discover available skills by listing subdirectories under the skill source directory.
fn discover_skills(skill_source_dir: &std::path::Path) -> Result<Vec<String>> {
    let mut skills = Vec::new();
    let entries = std::fs::read_dir(skill_source_dir).with_context(|| {
        format!("Failed to read skill directory: {}", skill_source_dir.display())
    })?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").exists() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                skills.push(name.to_string());
            }
        }
    }
    skills.sort();
    Ok(skills)
}

impl SkillDev {
    /// Run the skill dev command.
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let workspace_root = find_workspace_root()?;
        let skill_source_dir = workspace_root.join("crates/boundless-cli/skill");

        if !skill_source_dir.exists() {
            bail!(
                "Skill source directory not found at: {}\nAre you inside the Boundless repo?",
                skill_source_dir.display()
            );
        }

        let available_skills = discover_skills(&skill_source_dir)?;
        if available_skills.is_empty() {
            bail!("No skills found in {}", skill_source_dir.display());
        }

        // Determine which skills to link.
        let skills: Vec<String> = if let Some(ref name) = self.skill_name {
            if !available_skills.contains(name) {
                bail!(
                    "Unknown skill '{}'. Available skills: {}",
                    name,
                    available_skills.join(", ")
                );
            }
            vec![name.clone()]
        } else {
            available_skills.clone()
        };

        // Resolve the target directory.
        let base_dir = if let Some(ref path) = self.path {
            path.clone()
        } else {
            let format = Select::new("Select AI tool format:", TOOL_FORMATS.to_vec())
                .prompt()
                .context("Failed to get tool format selection")?;

            if self.global {
                let home = dirs::home_dir().context("Failed to get home directory")?;
                home.join(format.global_dir)
            } else {
                let cwd = std::env::current_dir().context("Failed to get current directory")?;
                cwd.join(format.local_dir)
            }
        };

        if self.clean {
            self.clean_symlinks(&base_dir, &skills)?;
        } else {
            self.create_symlinks(&base_dir, &skills, &skill_source_dir)?;
        }

        Ok(())
    }

    /// Create symlinks from the target directory to the skill source files.
    fn create_symlinks(
        &self,
        base_dir: &std::path::Path,
        skills: &[String],
        skill_source_dir: &std::path::Path,
    ) -> Result<()> {
        std::fs::create_dir_all(base_dir)
            .with_context(|| format!("Failed to create directory: {}", base_dir.display()))?;

        for skill_name in skills {
            let source = skill_source_dir.join(skill_name);
            let target = base_dir.join(skill_name);

            // If target already exists, handle it.
            if target.exists() || target.symlink_metadata().is_ok() {
                if target.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                    // Remove existing symlink.
                    std::fs::remove_file(&target).with_context(|| {
                        format!("Failed to remove existing symlink: {}", target.display())
                    })?;
                } else {
                    bail!(
                        "Target '{}' already exists and is not a symlink. \
                         Remove it manually or use a different --path.",
                        target.display()
                    );
                }
            }

            // Create the symlink.
            #[cfg(unix)]
            std::os::unix::fs::symlink(&source, &target).with_context(|| {
                format!("Failed to create symlink: {} -> {}", target.display(), source.display())
            })?;

            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&source, &target).with_context(|| {
                format!("Failed to create symlink: {} -> {}", target.display(), source.display())
            })?;

            println!(
                "{} Linked '{}': {} → {}",
                "✓".green().bold(),
                skill_name,
                target.display().to_string().dimmed(),
                source.display().to_string().cyan(),
            );
        }

        println!();
        println!(
            "{}",
            "Skills are now symlinked. Edit the source files and changes appear instantly."
                .dimmed()
        );
        println!("{}", "Run with --clean to remove the symlinks when done.".dimmed());
        println!();

        Ok(())
    }

    /// Remove symlinks created by `skill dev`.
    fn clean_symlinks(&self, base_dir: &std::path::Path, skills: &[String]) -> Result<()> {
        let mut removed = 0;

        for skill_name in skills {
            let target = base_dir.join(skill_name);

            if target.symlink_metadata().is_ok() {
                if target.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                    std::fs::remove_file(&target).with_context(|| {
                        format!("Failed to remove symlink: {}", target.display())
                    })?;
                    println!("{} Removed symlink: {}", "✓".green().bold(), target.display());
                    removed += 1;
                } else {
                    println!(
                        "{} Skipping '{}' — not a symlink",
                        "⚠".yellow().bold(),
                        target.display()
                    );
                }
            }
        }

        if removed == 0 {
            println!("{}", "No dev symlinks found to remove.".dimmed());
        } else {
            println!();
            println!("{} Cleaned {} symlink(s).", "✓".green().bold(), removed);
        }
        println!();

        Ok(())
    }
}
