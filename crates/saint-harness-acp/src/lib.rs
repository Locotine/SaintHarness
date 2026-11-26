use saint_harness_core::{DeliveryMode, InboxItemType};
use saint_harness_schema::{InteractionMode, RuntimeMode, SessionEvent};
use saint_harness_server::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

/// Thin ACP JSON-RPC 2.0 bridge over stdio (`initialize` with `mcpCapabilities.http: true`,
/// `session/new`, `session/load`, `session/prompt`, `session/cancel`, `session/set_config_option`,
/// and reverse `session/request_permission`).
pub struct AcpBridge {
    state: AppState,
}

impl AcpBridge {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    pub async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.unwrap_or(Value::Null);
        let res = match req.method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": "1",
                "agentInfo": {
                    "name": "SaintHarness",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "agentCapabilities": {
                    "loadSession": true,
                    "mcpCapabilities": { "http": true, "stdio": true }
                }
            })),
            "session/new" => self.handle_session_new(&req.params).await,
            "session/load" => self.handle_session_load(&req.params),
            "session/prompt" => self.handle_session_prompt(&req.params).await,
            "session/cancel" => self.handle_session_cancel(&req.params).await,
            "session/set_config_option" => self.handle_set_config(&req.params).await,
            other => Err(format!("method not found: {other}")),
        };

        match res {
            Ok(result) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(result),
                error: None,
            },
            Err(msg) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(json!({ "code": -32601, "message": msg })),
            },
        }
    }

    async fn handle_session_new(&self, params: &Value) -> Result<Value, String> {
        let cwd = params
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();
        let profile = params
            .get("modelProfile")
            .and_then(|v| v.as_str())
            .unwrap_or("qwen-3.8-27b")
            .to_string();
        let session_id = format!("sess_{}", Ulid::new());

        if let Some(mcp_servers) = params.get("mcpServers").and_then(|v| v.as_array()) {
            self.state
                .session_mcp_ram
                .insert(session_id.clone(), mcp_servers.clone());
        }

        self.state
            .store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: session_id.clone(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "acp session".to_string(),
                    agent_id: "build".to_string(),
                    model_profile: profile,
                    cwd: cwd.clone(),
                    workspace_root: cwd,
                    runtime_mode: RuntimeMode::ApprovalRequired,
                    interaction_mode: InteractionMode::Build,
                },
                None,
            )
            .await?;

        Ok(json!({ "sessionId": session_id }))
    }

    fn handle_session_load(&self, params: &Value) -> Result<Value, String> {
        let sid = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing sessionId".to_string())?;
        let sess = self
            .state
            .store
            .reader
            .get_session(sid)?
            .ok_or_else(|| format!("session `{sid}` not found"))?;
        Ok(json!({
            "sessionId": sess.id,
            "modelProfile": sess.model_profile,
            "cwd": sess.cwd
        }))
    }

    async fn handle_session_prompt(&self, params: &Value) -> Result<Value, String> {
        let sid = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing sessionId".to_string())?;
        let text = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let inbox_id = self
            .state
            .inbox
            .enqueue(
                sid,
                None,
                InboxItemType::Prompt,
                DeliveryMode::Queue,
                text,
                None,
            )
            .await?;
        Ok(json!({ "stopReason": "end_turn", "inboxId": inbox_id }))
    }

    async fn handle_session_cancel(&self, params: &Value) -> Result<Value, String> {
        let sid = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing sessionId".to_string())?;
        self.state.coordinator.interrupt(sid, false).await;
        Ok(json!({ "cancelled": true }))
    }

    async fn handle_set_config(&self, params: &Value) -> Result<Value, String> {
        let sid = params
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing sessionId".to_string())?;
        if let Some(profile) = params.get("value").and_then(|v| v.as_str()) {
            self.state
                .store
                .bus
                .publish_durable(
                    SessionEvent::SessionProfileSwitched {
                        session_id: sid.to_string(),
                        model_profile: profile.to_string(),
                        model_id: None,
                    },
                    None,
                )
                .await?;
        }
        Ok(json!({ "updated": true }))
    }

    /// Formats a reverse JSON-RPC `session/request_permission` request sent from SaintHarness to the ACP host.
    pub fn build_reverse_permission_request(
        request_id: &str,
        session_id: &str,
        title: &str,
        command: &str,
    ) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/request_permission",
            "params": {
                "sessionId": session_id,
                "title": title,
                "command": command,
                "options": [
                    { "optionId": "once", "name": "Approve once", "kind": "allow_once" },
                    { "optionId": "always", "name": "Always allow this session", "kind": "allow_always" },
                    { "optionId": "decline", "name": "Decline", "kind": "reject_once" },
                    { "optionId": "cancel", "name": "Cancel turn", "kind": "reject_always" }
                ]
            }
        })
    }

    pub async fn run_stdio_loop(&self) -> Result<(), String> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut lines = BufReader::new(stdin).lines();
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
                let resp = self.handle_request(req).await;
                let encoded = serde_json::to_string(&resp).map_err(|e| e.to_string())?;
                stdout
                    .write_all(format!("{encoded}\n").as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                stdout.flush().await.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acp_lifecycle_conformance_with_http_mcp_injection() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::bootstrap(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            tmp.path().join("acp_data"),
            "http://127.0.0.1:0".into(),
            None,
            true,
        )
        .await
        .unwrap();
        let bridge = AcpBridge::new(state.clone());

        let init = bridge
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(1)),
                method: "initialize".into(),
                params: json!({}),
            })
            .await;
        assert_eq!(
            init.result.unwrap()["agentCapabilities"]["mcpCapabilities"]["http"],
            json!(true)
        );

        let created = bridge
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(2)),
                method: "session/new".into(),
                params: json!({
                    "cwd": tmp.path().to_string_lossy(),
                    "mcpServers": [{ "name": "saint", "type": "http", "url": "http://127.0.0.1:9191/mcp" }]
                }),
            })
            .await;
        let sid = created.result.unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(state.session_mcp_ram.contains_key(&sid));

        let loaded = bridge
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(3)),
                method: "session/load".into(),
                params: json!({ "sessionId": sid }),
            })
            .await;
        assert!(loaded.error.is_none());

        let prompted = bridge
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: Some(json!(4)),
                method: "session/prompt".into(),
                params: json!({ "sessionId": sid, "prompt": "Check WAL writer" }),
            })
            .await;
        assert_eq!(prompted.result.unwrap()["stopReason"], json!("end_turn"));

        let rev = AcpBridge::build_reverse_permission_request(
            "perm_1",
            &sid,
            "Approve this command?",
            "cargo test",
        );
        assert_eq!(rev["method"], json!("session/request_permission"));
    }
}
