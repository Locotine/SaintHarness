use saint_harness_schema::{McpExposure, McpServerSpec, ToolDef};

#[derive(Debug, Clone)]
pub struct RoutedMcpCatalog {
    pub direct_tools: Vec<ToolDef>,
    pub deferred_tools: Vec<ToolDef>,
    pub codemode_tools: Vec<ToolDef>,
}

/// Routes MCP server tools across the 3 exposure modes (`Direct`, `Deferred`, `CodeMode`):
/// - `Direct` (e.g. trusted `"saint"` gateway) exposes tools directly as `mcp__<server>__<tool>` with `server_name`.
/// - `Deferred` exposes tools with `defer_loading = true` (Claude Tool Search).
/// - `CodeMode` keeps MCP tools out of the direct tool list so small models (Qwen 27B) stay `<= max_direct_tools`.
pub fn route_mcp_tools(
    servers: &[McpServerSpec],
    discovered: &[(String, Vec<ToolDef>)],
    max_direct_tools: usize,
) -> RoutedMcpCatalog {
    let mut direct_tools = Vec::new();
    let mut deferred_tools = Vec::new();
    let mut codemode_tools = Vec::new();

    for (server_name, tools) in discovered {
        let exposure = servers
            .iter()
            .find(|s| &s.name == server_name)
            .map(|s| s.exposure)
            .unwrap_or(McpExposure::CodeMode);

        for t in tools {
            let mut canonical = t.clone();
            if !canonical.name.starts_with("mcp__") {
                canonical.name = format!("mcp__{server_name}__{}", canonical.name);
            }
            canonical.server_name = Some(server_name.clone());

            match exposure {
                McpExposure::Direct => {
                    canonical.defer_loading = false;
                    direct_tools.push(canonical);
                }
                McpExposure::Deferred => {
                    canonical.defer_loading = true;
                    deferred_tools.push(canonical);
                }
                McpExposure::CodeMode => {
                    codemode_tools.push(canonical);
                }
            }
        }
    }

    // Overflow safeguard if too many non-Direct tools would exceed cap
    if direct_tools.len() > max_direct_tools * 4 {
        let overflow = direct_tools.split_off(max_direct_tools * 4);
        codemode_tools.extend(overflow);
    }

    RoutedMcpCatalog {
        direct_tools,
        deferred_tools,
        codemode_tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use saint_harness_schema::{McpTransport, SecretString};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn fifty_codemode_tools_stay_hidden_while_saint_direct_exposes_canonical_names() {
        let servers = vec![
            McpServerSpec {
                name: "bulk".into(),
                transport: McpTransport::Stdio {
                    command: "bulk-mcp".into(),
                    args: vec![],
                    env: BTreeMap::new(),
                },
                exposure: McpExposure::CodeMode,
                auto_approve: false,
            },
            McpServerSpec {
                name: "saint".into(),
                transport: McpTransport::StreamableHttp {
                    url: "http://127.0.0.1:4123/mcp".into(),
                    bearer_token: Some(SecretString::new("ram-only-bearer")),
                    headers: BTreeMap::new(),
                },
                exposure: McpExposure::Direct,
                auto_approve: true,
            },
        ];

        let bulk_tools: Vec<ToolDef> = (0..50)
            .map(|i| ToolDef {
                name: format!("op_{i}"),
                description: "bulk".into(),
                input_schema: json!({"type":"object"}),
                defer_loading: false,
                server_name: None,
            })
            .collect();
        let saint_tools = vec![ToolDef {
            name: "saint_context".into(),
            description: "ctx".into(),
            input_schema: json!({"type":"object"}),
            defer_loading: false,
            server_name: None,
        }];

        let routed = route_mcp_tools(
            &servers,
            &[("bulk".into(), bulk_tools), ("saint".into(), saint_tools)],
            8,
        );
        assert_eq!(routed.codemode_tools.len(), 50);
        assert_eq!(routed.direct_tools.len(), 1);
        assert_eq!(routed.direct_tools[0].name, "mcp__saint__saint_context");
        assert_eq!(routed.direct_tools[0].server_name.as_deref(), Some("saint"));
    }
}
