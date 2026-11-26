use saint_harness_schema::ToolDef;

pub const DEFAULT_CATALOG_BUDGET_TOKENS: usize = 2_000;

#[derive(Debug, Clone)]
pub struct CodeModeCatalog {
    tools: Vec<ToolDef>,
}

impl CodeModeCatalog {
    pub fn new(mut tools: Vec<ToolDef>) -> Self {
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        Self { tools }
    }

    /// Renders a compact TypeScript-like inline catalog capped at `budget_tokens`.
    pub fn render_inline_catalog(&self, budget_tokens: usize) -> String {
        let max_chars = budget_tokens.saturating_mul(4);
        let mut out = String::from("// Available tools via tools.call(namespace, name, args)\n");
        for (included, tool) in self.tools.iter().enumerate() {
            let ns = tool.server_name.as_deref().unwrap_or("core");
            let desc: String = tool.description.chars().take(80).collect();
            let line = format!("// {ns}.{}: {desc}\n", tool.name);
            if out.len() + line.len() > max_chars {
                let remaining = self.tools.len().saturating_sub(included);
                out.push_str(&format!(
                    "// ... {remaining} additional tools available via tools.search(query)\n"
                ));
                break;
            }
            out.push_str(&line);
        }
        out
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<&ToolDef> {
        let q = query.to_ascii_lowercase();
        self.tools
            .iter()
            .filter(|t| {
                t.name.to_ascii_lowercase().contains(&q)
                    || t.description.to_ascii_lowercase().contains(&q)
                    || t.server_name
                        .as_deref()
                        .is_some_and(|s| s.to_ascii_lowercase().contains(&q))
            })
            .take(limit)
            .collect()
    }
}
