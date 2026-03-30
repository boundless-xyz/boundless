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
//!
//! Skills are discovered dynamically from either:
//! - The local repo's `.claude/skills/` directory (if running inside the Boundless repo)
//! - GitHub at `https://github.com/boundless-xyz/boundless` (if running outside the repo)

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use inquire::Select;
use crate::config::GlobalConfig;

const GITHUB_REPO: &str = "boundless-xyz/boundless";
const GITHUB_BRANCH: &str = "main";
const SKILLS_PATH: &str = ".claude/skills";

/// A discovered skill with its name, description, and file contents.
struct Skill {
    name: String,
    description: String,
    /// Files to install: (relative_path, content).
    files: Vec<(String, String)>,
}

impl fmt::Display for Skill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.name, self.description)
    }
}

/// Extract the `description` field from SKILL.md YAML frontmatter.
fn parse_description(skill_md: &str) -> Option<String> {
    let content = skill_md.strip_prefix("---")?;
    let end = content.find("---")?;
    let frontmatter = &content[..end];
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(desc) = line.strip_prefix("description:") {
            return Some(desc.trim().to_string());
        }
    }
    None
}

// -- Local repo discovery --

/// Walk up from `start` looking for the Boundless repo root (has `.claude/skills/` and `Cargo.toml`
/// with `[workspace]`).
fn find_repo_skills_dir() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let skills_dir = dir.join(SKILLS_PATH);
        let cargo_toml = dir.join("Cargo.toml");
        if skills_dir.is_dir() && cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml).ok()?;
            if content.contains("[workspace]") {
                return Some(skills_dir);
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Discover skills from the local `.claude/skills/` directory.
fn discover_local_skills(skills_dir: &Path) -> Result<Vec<Skill>> {
    let mut skills = Vec::new();

    for entry in std::fs::read_dir(skills_dir)
        .with_context(|| format!("Failed to read {}", skills_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md_path = path.join("SKILL.md");
        if !skill_md_path.exists() {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("Invalid skill directory name")?
            .to_string();

        // Collect all files recursively.
        let mut files = Vec::new();
        collect_files_recursive(&path, &path, &mut files)?;

        let skill_md = std::fs::read_to_string(&skill_md_path)
            .with_context(|| format!("Failed to read {}", skill_md_path.display()))?;
        let description = parse_description(&skill_md).unwrap_or_default();

        skills.push(Skill { name, description, files });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Recursively collect all files under `dir`, storing paths relative to `base`.
fn collect_files_recursive(base: &Path, dir: &Path, files: &mut Vec<(String, String)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(base, &path, files)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .context("Failed to compute relative path")?
                .to_string_lossy()
                .to_string();
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            files.push((rel, content));
        }
    }
    Ok(())
}

// -- GitHub discovery --

/// Discover skills from GitHub by listing `.claude/skills/` in the repo.
async fn discover_github_skills(client: &reqwest::Client) -> Result<Vec<Skill>> {
    // Use the Git Trees API to get the entire skills directory in one request.
    // First, we need the tree SHA for the branch.
    let branch_url = format!(
        "https://api.github.com/repos/{}/git/ref/heads/{}",
        GITHUB_REPO, GITHUB_BRANCH
    );
    let branch: serde_json::Value = github_get_json(client, &branch_url).await
        .context("Failed to fetch branch ref from GitHub")?;
    let commit_sha = branch["object"]["sha"]
        .as_str()
        .context("Could not find commit SHA")?;

    // Get the commit to find the root tree.
    let commit_url = format!(
        "https://api.github.com/repos/{}/git/commits/{}",
        GITHUB_REPO, commit_sha
    );
    let commit: serde_json::Value = github_get_json(client, &commit_url).await?;
    let root_tree_sha = commit["tree"]["sha"]
        .as_str()
        .context("Could not find root tree SHA")?;

    // Get the full repo tree recursively (single API call).
    let tree_url = format!(
        "https://api.github.com/repos/{}/git/trees/{}?recursive=1",
        GITHUB_REPO, root_tree_sha
    );
    let tree: serde_json::Value = github_get_json(client, &tree_url).await?;
    let entries = tree["tree"]
        .as_array()
        .context("Invalid tree response")?;

    // Filter to files under .claude/skills/ and group by skill name.
    let prefix = format!("{}/", SKILLS_PATH);
    let mut skill_files: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

    for entry in entries {
        let path = entry["path"].as_str().unwrap_or("");
        let entry_type = entry["type"].as_str().unwrap_or("");
        if entry_type == "blob" && path.starts_with(&prefix) {
            let rel = &path[prefix.len()..];
            if let Some(slash) = rel.find('/') {
                let skill_name = &rel[..slash];
                let file_path = &rel[slash + 1..];
                skill_files
                    .entry(skill_name.to_string())
                    .or_default()
                    .push(file_path.to_string());
            }
        }
    }

    if skill_files.is_empty() {
        bail!("No skills found in the Boundless repository");
    }

    // Fetch file contents for each skill using raw.githubusercontent.com.
    let mut skills = Vec::new();
    for (skill_name, file_paths) in &skill_files {
        if !file_paths.iter().any(|f| f == "SKILL.md") {
            continue;
        }
        match fetch_github_skill_files(client, skill_name, file_paths).await {
            Ok(skill) => skills.push(skill),
            Err(e) => {
                eprintln!(
                    "{} Skipping skill '{}': {}",
                    "⚠".yellow().bold(),
                    skill_name,
                    e
                );
            }
        }
    }

    Ok(skills)
}

/// Fetch a JSON response from the GitHub API.
async fn github_get_json(client: &reqwest::Client, url: &str) -> Result<serde_json::Value> {
    client
        .get(url)
        .header("User-Agent", "boundless-cli")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .context("Failed to parse GitHub JSON response")
}

/// Fetch a single skill's file contents from raw.githubusercontent.com.
async fn fetch_github_skill_files(
    client: &reqwest::Client,
    skill_name: &str,
    file_paths: &[String],
) -> Result<Skill> {
    let mut files = Vec::new();
    let mut description = String::new();

    for file_path in file_paths {
        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}/{}",
            GITHUB_REPO, GITHUB_BRANCH, SKILLS_PATH, skill_name, file_path
        );
        let content = client
            .get(&raw_url)
            .header("User-Agent", "boundless-cli")
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("Failed to fetch {}/{}", skill_name, file_path))?
            .text()
            .await?;

        if file_path == "SKILL.md" {
            description = parse_description(&content).unwrap_or_default();
        }

        files.push((file_path.clone(), content));
    }

    Ok(Skill {
        name: skill_name.to_string(),
        description,
        files,
    })
}

/// Where skills come from.
enum SkillSource {
    /// Local `.claude/skills/` directory in the repo.
    Local(PathBuf),
    /// Fetched from GitHub.
    Remote,
}

impl fmt::Display for SkillSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(path) => write!(f, "local ({})", path.display()),
            Self::Remote => write!(f, "github.com/{} ({})", GITHUB_REPO, GITHUB_BRANCH),
        }
    }
}

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
        // Discover skills: prefer local repo, fall back to GitHub.
        let (source, skills) = self.discover_skills().await?;

        println!(
            "  {} {}",
            "Source:".dimmed(),
            source.to_string().dimmed()
        );

        if skills.is_empty() {
            bail!("No skills found");
        }

        // Handle --list: print available skills and return.
        if self.list {
            println!();
            println!("{}", "Available skills:".bold());
            println!();
            for skill in &skills {
                println!(
                    "  {} {}",
                    skill.name.bold(),
                    format!("- {}", skill.description).dimmed()
                );
            }
            println!();
            return Ok(());
        }

        // Determine which skills to install.
        let skills_to_install: Vec<&Skill> = if self.all {
            skills.iter().collect()
        } else if let Some(ref name) = self.skill_name {
            let skill = skills.iter().find(|s| s.name == *name);
            match skill {
                Some(s) => vec![s],
                None => {
                    let available: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
                    bail!(
                        "Unknown skill '{}'. Available skills: {}",
                        name,
                        available.join(", ")
                    );
                }
            }
        } else {
            // Interactive selection.
            let options: Vec<String> = skills.iter().map(|s| s.to_string()).collect();
            let selection = Select::new("Select a skill to install:", options)
                .prompt()
                .context("Failed to get skill selection")?;
            let skill = skills
                .iter()
                .find(|s| s.to_string() == selection)
                .context("Failed to find selected skill")?;
            vec![skill]
        };

        // Resolve the base install directory.
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

        // Install each skill.
        for skill in &skills_to_install {
            self.install_skill(skill, &base_dir)?;
        }

        Ok(())
    }

    /// Discover available skills from local repo or GitHub.
    async fn discover_skills(&self) -> Result<(SkillSource, Vec<Skill>)> {
        // Try local repo first.
        if let Some(skills_dir) = find_repo_skills_dir() {
            let skills = discover_local_skills(&skills_dir)?;
            return Ok((SkillSource::Local(skills_dir), skills));
        }

        // Fall back to GitHub.
        eprintln!(
            "{}",
            "Not inside the Boundless repo — fetching skills from GitHub...".dimmed()
        );
        let client = reqwest::Client::new();
        let skills = discover_github_skills(&client).await?;
        Ok((SkillSource::Remote, skills))
    }

    /// Install a single skill to the target directory.
    fn install_skill(&self, skill: &Skill, base_dir: &Path) -> Result<()> {
        let install_dir = base_dir.join(&skill.name);

        // Create all needed directories.
        for (rel_path, _) in &skill.files {
            if let Some(parent) = Path::new(rel_path).parent() {
                if !parent.as_os_str().is_empty() {
                    let dir = install_dir.join(parent);
                    std::fs::create_dir_all(&dir)
                        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
                }
            }
        }
        std::fs::create_dir_all(&install_dir)
            .with_context(|| format!("Failed to create directory: {}", install_dir.display()))?;

        // Write all files.
        for (rel_path, content) in &skill.files {
            let file_path = install_dir.join(rel_path);
            std::fs::write(&file_path, content)
                .with_context(|| format!("Failed to write file: {}", file_path.display()))?;
        }

        // Print success.
        println!();
        println!(
            "{} Skill '{}' installed successfully!",
            "✓".green().bold(),
            skill.name
        );
        println!();

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
        for (name, _) in &skill.files {
            println!("    {} {}", "•".dimmed(), name);
        }
        println!();

        Ok(())
    }
}
