// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use glob::glob;
use regex::Regex;
use std::{
    collections::HashMap,
    env,
    fmt::{self, Write},
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
struct Level {
    nested: HashMap<String, Level>,
    files: Vec<PathBuf>,
}

#[derive(Debug)]
struct RustSnippet {
    content: String,
    should_run: bool,
}

fn main() {
    let home = env::var("CARGO_MANIFEST_DIR").unwrap();
    let mut level = Level::new();

    for root_dir in ["documentation"] {
        let pattern = format!("{home}/../../{root_dir}/site/pages/**/*.mdx");
        let base = format!("{home}/../../{root_dir}",);
        let base = Path::new(&base).canonicalize().unwrap();

        for entry in glob(&pattern).unwrap() {
            let path = entry.unwrap();
            let path = Path::new(&path).canonicalize().unwrap();
            println!("cargo:rerun-if-changed={}", path.display());

            let rel = path.strip_prefix(&base).unwrap();
            let mut parts = vec![];
            for part in rel {
                parts.push(part.to_str().unwrap());
            }
            level.insert(path.clone(), &parts[..]);
        }
    }

    let out = format!("{}/doctests.rs", env::var("OUT_DIR").unwrap());
    fs::write(out, level.to_string()).unwrap();
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dst = String::new();
        self.write_inner(&mut dst, 0)?;
        f.write_str(&dst)?;
        Ok(())
    }
}

impl Level {
    fn new() -> Level {
        Level { nested: HashMap::new(), files: vec![] }
    }

    fn insert(&mut self, path: PathBuf, rel: &[&str]) {
        if rel.iter().any(|part| part.starts_with('_')) {
            return;
        }
        if rel.len() == 1 {
            self.files.push(path);
        } else {
            let nested = self.nested.entry(rel[0].to_string()).or_insert(Level::new());
            nested.insert(path, &rel[1..]);
        }
    }

    fn write_into(&self, dst: &mut String, name: &str, level: usize) -> fmt::Result {
        self.write_space(dst, level);
        let name = name.replace(['-', '.'], "_");
        writeln!(dst, "#[allow(non_snake_case, unused)]")?;
        writeln!(dst, "pub mod {name} {{")?;
        self.write_inner(dst, level + 1)?;
        self.write_space(dst, level);
        writeln!(dst, "}}")?;
        Ok(())
    }

    fn write_inner(&self, dst: &mut String, level: usize) -> fmt::Result {
        for (name, nested) in &self.nested {
            nested.write_into(dst, name, level)?;
        }

        self.write_space(dst, level);
        for file in &self.files {
            let stem = Path::new(file).file_stem().unwrap().to_str().unwrap().replace('-', "_");
            let content = fs::read_to_string(file).expect("Failed to read file");
            let rust_snippets = self.extract_rust_snippets(&content);

            if rust_snippets.is_empty() {
                continue;
            }

            let mut seen_snippets = Vec::new();
            for snippet in rust_snippets {
                if !seen_snippets.iter().any(|s: &RustSnippet| s.content == snippet.content) {
                    seen_snippets.push(snippet);
                }
            }

            for (i, snippet) in seen_snippets.iter().enumerate() {
                self.write_space(dst, level);
                writeln!(dst, "#[test]")?;
                if !snippet.should_run {
                    self.write_space(dst, level);
                    writeln!(dst, "#[ignore]")?;
                }
                self.write_space(dst, level);
                writeln!(dst, "fn {}_md_{}_test() {{", stem, i)?;
                // Indent the code content
                for line in snippet.content.lines() {
                    self.write_space(dst, level + 1);
                    writeln!(dst, "{}", line)?;
                }
                self.write_space(dst, level);
                writeln!(dst, "}}")?;
            }
        }
        Ok(())
    }

    fn normalize_whitespace(&self, text: &str) -> String {
        let lines: Vec<_> = text
            .lines()
            .filter(|line| !line.contains("....")) // Filter out lines containing ....
            .collect();

        if lines.is_empty() {
            return String::new();
        }

        let min_indent = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);

        lines
            .iter()
            .map(|line| {
                if line.trim().is_empty() {
                    ""
                } else {
                    &line[min_indent.min(line.len() - line.trim_start().len())..]
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn process_code_block(&self, code: &str) -> String {
        let mut processed_lines = Vec::new();
        let normalized = self.normalize_whitespace(code);

        for line in normalized.lines() {
            let trimmed = line.trim_start();

            if trimmed.starts_with("# ") {
                let content = trimmed[2..].trim_start();
                if !content.is_empty() {
                    processed_lines.push(content.to_string());
                }
            } else if !trimmed.starts_with('#') {
                let line = line.replace("// [!code focus]", "").trim_end().to_string();
                let line = line.replace("showLineNumbers", "").trim_end().to_string();
                if !line.trim().is_empty() {
                    processed_lines.push(line);
                }
            }
        }

        processed_lines.join("\n")
    }

    fn extract_rust_snippets(&self, content: &str) -> Vec<RustSnippet> {
        let re = Regex::new(r"```rust(?:\s+(ignore|no_run))?\s*((?:.|\n)*?)```").unwrap();
        re.captures_iter(content)
            .filter_map(|cap| {
                let flag = cap.get(1).map(|m| m.as_str());
                let code = cap.get(2)?.as_str().trim();

                match flag {
                    Some("ignore") => None,
                    Some("no_run") => Some(RustSnippet {
                        content: self.process_code_block(code),
                        should_run: false,
                    }),
                    _ => Some(RustSnippet {
                        content: self.process_code_block(code),
                        should_run: true,
                    }),
                }
            })
            .collect()
    }

    fn write_space(&self, dst: &mut String, level: usize) {
        dst.push_str(&" ".repeat(level));
    }
}
