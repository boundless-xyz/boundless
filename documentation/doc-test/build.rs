// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    collections::HashMap,
    env,
    fmt::{self, Write},
    fs,
    path::{Path, PathBuf},
};

use glob::glob;
use regex::Regex;

#[derive(Debug)]
struct Level {
    nested: HashMap<String, Level>,
    files: Vec<PathBuf>,
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
        writeln!(dst, "pub mod {name} {{",)?;
        self.write_inner(dst, level + 1)?;
        self.write_space(dst, level);
        writeln!(dst, "}}")?;

        Ok(())
    }

    // fn sanitize_doc(&self, content: &str) -> String {
    //     content
    //         .split('\n')
    //         .filter(|line| {
    //             !line.starts_with('#') && !line.contains("@risc0/") && line.trim().len() > 0
    //         })
    //         .map(|line| line.replace(|c| c == '[' || c == ']' || c == '&' || c == '/', ""))
    //         .collect::<Vec<_>>()
    //         .join("\n")
    //         .trim()
    //         .to_string()
    // }

    fn write_inner(&self, dst: &mut String, level: usize) -> fmt::Result {
        for (name, nested) in &self.nested {
            nested.write_into(dst, name, level)?;
        }

        self.write_space(dst, level);

        for file in &self.files {
            let stem = Path::new(file).file_stem().unwrap().to_str().unwrap().replace('-', "_");
            let content = fs::read_to_string(file).expect("Failed to read file");
            let processed_content = self.remove_footnotes(&content);
            let sanitized_content = processed_content; //self.sanitize_doc(&processed_content);

            // Skip empty documents
            if sanitized_content.trim().is_empty() {
                continue;
            }

            self.write_space(dst, level);
            writeln!(dst, "#[doc = r##\"{}\"##]", sanitized_content)?;
            self.write_space(dst, level);
            writeln!(dst, "pub fn {}_md() {{}}", stem)?;
        }

        Ok(())
    }

    fn remove_footnotes(&self, content: &str) -> String {
        let patterns = [
            (r"(?s)^---.*?---\s*", ""),
            (r"```rust(?!\s* \s*(?:ignore|no_run))\b", "```rust ignore"),
            (r"```solidity.*?```", ""),
            (r"(?ms):::\w+(?:\[.*?\])?\n.*?\n:::", ""),
            (r"<[^>]+>.*?</[^>]+>|<[^>]+/>", ""),
            (r"(?m)^>\s.*$", ""),
            (r"\n{3,}", "\n\n"),
        ];

        patterns.iter().fold(content.to_string(), |acc, (pattern, replacement)| {
            Regex::new(pattern)
                .map(|re| re.replace_all(&acc, *replacement).to_string())
                .unwrap_or(acc)
        })
    }

    fn write_space(&self, dst: &mut String, level: usize) {
        for _ in 0..level {
            dst.push_str("    ");
        }
    }
}
