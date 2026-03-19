use crate::config;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

/// A single persisted workspace permission rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Tool name (e.g. "bash") or "directory" for dir-based approvals.
    pub tool: String,
    /// Glob patterns — empty means "allow all" for this tool.
    #[serde(default)]
    pub patterns: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    rules: Vec<Rule>,
}

fn workspace_dir(cwd: &str) -> PathBuf {
    let hash = format!("{:x}", Sha256::digest(cwd.as_bytes()));
    config::state_dir().join("workspaces").join(&hash[..16])
}

fn permissions_path(cwd: &str) -> PathBuf {
    workspace_dir(cwd).join("permissions.json")
}

pub fn load(cwd: &str) -> Vec<Rule> {
    let path = permissions_path(cwd);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let store: Store = serde_json::from_str(&contents).unwrap_or_default();
    store.rules
}

pub fn save(cwd: &str, rules: &[Rule]) {
    let path = permissions_path(cwd);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let store = Store {
        rules: rules.to_vec(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&store) {
        let _ = std::fs::write(&path, json);
    }
}

/// Add a tool-level approval rule for the workspace.
pub fn add_tool(cwd: &str, tool: &str, patterns: Vec<String>) {
    let mut rules = load(cwd);
    // Merge with existing rule for this tool if present.
    if let Some(existing) = rules.iter_mut().find(|r| r.tool == tool) {
        if patterns.is_empty() || existing.patterns.is_empty() {
            existing.patterns.clear(); // "allow all" wins
        } else {
            for p in &patterns {
                if !existing.patterns.contains(p) {
                    existing.patterns.push(p.clone());
                }
            }
        }
    } else {
        rules.push(Rule {
            tool: tool.to_string(),
            patterns,
        });
    }
    save(cwd, &rules);
}

/// Add a directory-level approval rule for the workspace.
pub fn add_dir(cwd: &str, dir: &str) {
    let mut rules = load(cwd);
    let already = rules
        .iter()
        .any(|r| r.tool == "directory" && r.patterns.iter().any(|p| p == dir));
    if !already {
        rules.push(Rule {
            tool: "directory".into(),
            patterns: vec![dir.to_string()],
        });
    }
    save(cwd, &rules);
}

/// Build the auto_approved and auto_approved_dirs from workspace rules.
pub fn into_approvals(rules: &[Rule]) -> (HashMap<String, Vec<glob::Pattern>>, Vec<PathBuf>) {
    let mut tool_map: HashMap<String, Vec<glob::Pattern>> = HashMap::new();
    let mut dirs = Vec::new();
    for rule in rules {
        if rule.tool == "directory" {
            for p in &rule.patterns {
                dirs.push(PathBuf::from(p));
            }
        } else {
            let compiled: Vec<glob::Pattern> = rule
                .patterns
                .iter()
                .filter(|p| *p != "*")
                .filter_map(|p| glob::Pattern::new(p).ok())
                .collect();
            tool_map
                .entry(rule.tool.clone())
                .or_default()
                .extend(compiled);
        }
    }
    (tool_map, dirs)
}
