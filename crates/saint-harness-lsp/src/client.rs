use dashmap::DashMap;
use serde_json::{Value, json};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, oneshot};

pub struct JsonRpcLspClient {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<DashMap<i64, oneshot::Sender<Value>>>,
    diagnostics: Arc<DashMap<String, Vec<lsp_types::Diagnostic>>>,
    next_id: AtomicI64,
    _child: Arc<Mutex<Child>>,
}

impl JsonRpcLspClient {
    pub async fn spawn(command: &str, args: &[String], root_uri: &str) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn LSP server '{command}': {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "LSP server missing stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "LSP server missing stdout".to_string())?;

        let pending: Arc<DashMap<i64, oneshot::Sender<Value>>> = Arc::new(DashMap::new());
        let diagnostics: Arc<DashMap<String, Vec<lsp_types::Diagnostic>>> =
            Arc::new(DashMap::new());

        let pending_reader = pending.clone();
        let diag_reader = diagnostics.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            while let Some(msg) = read_lsp_frame(&mut reader).await {
                if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
                    if let Some((_, tx)) = pending_reader.remove(&id) {
                        let _ = tx.send(msg);
                    }
                } else if msg.get("method").and_then(|v| v.as_str())
                    == Some("textDocument/publishDiagnostics")
                    && let Some(params) = msg.get("params")
                    && let Ok(parsed) = serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(
                        params.clone(),
                    )
                {
                    diag_reader.insert(parsed.uri.as_str().to_string(), parsed.diagnostics);
                }
            }
        });

        let client = Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            diagnostics,
            next_id: AtomicI64::new(1),
            _child: Arc::new(Mutex::new(child)),
        };

        let _ = client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "capabilities": {}
                }),
                Duration::from_millis(1500),
            )
            .await?;
        client.notify("initialized", json!({})).await?;
        Ok(client)
    }

    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);

        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.write_frame(&payload).await?;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp.get("result").cloned().unwrap_or(Value::Null)),
            Ok(Err(_)) => Err("LSP server closed connection".to_string()),
            Err(_) => {
                self.pending.remove(&id);
                Err(format!(
                    "LSP request '{method}' timed out after {timeout:?}"
                ))
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.write_frame(&payload).await
    }

    pub fn cached_diagnostics(&self, uri: &str) -> Vec<lsp_types::Diagnostic> {
        self.diagnostics
            .get(uri)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    async fn write_frame(&self, payload: &Value) -> Result<(), String> {
        let body = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(header.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())
    }
}

async fn read_lsp_frame<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Option<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.ok()?;
        if n == 0 {
            return None;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let len = content_length?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}
