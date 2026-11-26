use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredSkill {
    pub id: String,
    pub description: String,
    pub path: PathBuf,
}

/// Scans `.saint-harness/skills/` up to depth 6 for `SKILL.md` descriptors.
pub fn discover_skills(dirs: &[PathBuf]) -> Vec<DiscoveredSkill> {
    let mut skills = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        for entry in WalkBuilder::new(dir)
            .max_depth(Some(6))
            .hidden(false)
            .build()
            .flatten()
        {
            let p = entry.path();
            if p.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
                continue;
            }
            let Ok(body) = fs::read_to_string(p) else {
                continue;
            };
            let id = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("skill")
                .to_string();
            let description = extract_frontmatter_field(&body, "description")
                .unwrap_or_else(|| body.lines().next().unwrap_or("").to_string());
            skills.push(DiscoveredSkill {
                id,
                description,
                path: p.to_path_buf(),
            });
        }
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills
}

pub fn render_available_skills_xml(skills: &[DiscoveredSkill]) -> String {
    let mut out = String::from("<available_skills>\n");
    for s in skills {
        out.push_str(&format!("- {}: {}\n", s.id, s.description));
    }
    out.push_str("</available_skills>");
    out
}

fn extract_frontmatter_field(body: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in body.lines().take(12) {
        if let Some(rest) = line.trim().strip_prefix(&prefix) {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

pub fn default_skill_dirs(workspace_root: &Path, home_dir: &Path) -> Vec<PathBuf> {
    vec![
        home_dir.join(".saint-harness").join("skills"),
        workspace_root.join(".saint-harness").join("skills"),
    ]
}
