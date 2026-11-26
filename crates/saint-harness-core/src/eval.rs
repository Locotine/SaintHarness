use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalTaskSpec {
    pub id: String,
    pub prompt: String,
    pub verify_command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSuiteSpec {
    pub name: String,
    pub tasks: Vec<EvalTaskSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalMatrixRow {
    pub profile: String,
    pub edit_strategy: String,
    pub repeat: usize,
    pub pass_at_1: f64,
    pub cache_hit_ratio: f64,
    pub prefix_preserved_rate: f64,
    pub edit_first_try_rate: f64,
    pub lsp_errors_caught: usize,
    pub compression_ratio: f64,
    pub cost_per_resolved_task: f64,
    pub avg_wall_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    pub suite: String,
    pub repeat: usize,
    pub rows: Vec<EvalMatrixRow>,
}

impl EvalReport {
    pub fn to_markdown_table(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "Profile           Edit         Pass@1   Cache Hit   1st-Try Edit   Cost/Task   Wall\n",
        );
        out.push_str(
            "─────────────────────────────────────────────────────────────────────────────────────\n",
        );
        for r in &self.rows {
            out.push_str(&format!(
                "{:<17} {:<12} {:>5.1}%   {:>7.1}%    {:>9.1}%     ${:<9.3} {:>4.1}s\n",
                r.profile,
                r.edit_strategy,
                r.pass_at_1 * 100.0,
                r.cache_hit_ratio * 100.0,
                r.edit_first_try_rate * 100.0,
                r.cost_per_resolved_task,
                r.avg_wall_secs
            ));
        }
        out
    }
}

pub fn load_eval_suite(path: &std::path::Path) -> Result<EvalSuiteSpec, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    } else {
        serde_yaml::from_str(&raw).map_err(|e| e.to_string())
    }
}
