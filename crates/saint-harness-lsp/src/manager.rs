use crate::client::JsonRpcLspClient;
use crate::diagnostics::collect_error_diagnostics;
use crate::symbols::{file_to_uri, query_find_callers, query_symbol_read};
use async_trait::async_trait;
use dashmap::DashMap;
use saint_harness_schema::{DiagnosticItem, LspPort, SymbolLocation};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct LazyLspManager {
    workspace_root: PathBuf,
    commands: BTreeMap<String, (String, Vec<String>)>,
    clients: DashMap<String, Arc<JsonRpcLspClient>>,
}

impl LazyLspManager {
    pub fn new(workspace_root: PathBuf) -> Self {
        let mut commands = BTreeMap::new();
        commands.insert("rs".to_string(), ("rust-analyzer".to_string(), vec![]));
        commands.insert(
            "ts".to_string(),
            (
                "typescript-language-server".to_string(),
                vec!["--stdio".to_string()],
            ),
        );
        commands.insert(
            "tsx".to_string(),
            (
                "typescript-language-server".to_string(),
                vec!["--stdio".to_string()],
            ),
        );
        commands.insert(
            "py".to_string(),
            (
                "pyright-langserver".to_string(),
                vec!["--stdio".to_string()],
            ),
        );
        commands.insert("go".to_string(), ("gopls".to_string(), vec![]));
        Self {
            workspace_root,
            commands,
            clients: DashMap::new(),
        }
    }

    pub fn override_server(&mut self, ext: &str, command: String, args: Vec<String>) {
        self.commands.insert(ext.to_string(), (command, args));
    }

    async fn get_or_spawn(&self, path: &Path) -> Result<Arc<JsonRpcLspClient>, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| "file has no extension for LSP".to_string())?;
        if let Some(existing) = self.clients.get(ext) {
            return Ok(existing.value().clone());
        }
        let (cmd, args) = self
            .commands
            .get(ext)
            .cloned()
            .ok_or_else(|| format!("no LSP server configured for .{ext}"))?;
        let root_uri = file_to_uri(&self.workspace_root)?;
        let client = Arc::new(JsonRpcLspClient::spawn(&cmd, &args, &root_uri).await?);
        self.clients.insert(ext.to_string(), client.clone());
        Ok(client)
    }

    async fn sync_document(
        &self,
        client: &JsonRpcLspClient,
        path: &Path,
    ) -> Result<String, String> {
        let uri = file_to_uri(path)?;
        let text = fs::read_to_string(path).unwrap_or_default();
        let lang_id = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("plaintext");
        client
            .notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": lang_id,
                        "version": 1,
                        "text": text
                    }
                }),
            )
            .await?;
        Ok(uri)
    }
}

#[async_trait]
impl LspPort for LazyLspManager {
    async fn diagnostics_after_write(&self, path: &Path) -> Vec<DiagnosticItem> {
        let Ok(client) = self.get_or_spawn(path).await else {
            return Vec::new();
        };
        let Ok(uri) = self.sync_document(&client, path).await else {
            return Vec::new();
        };
        collect_error_diagnostics(&client, path, &uri).await
    }

    async fn symbol_read(&self, path: &Path, symbol_name: &str) -> Result<SymbolLocation, String> {
        let client = self.get_or_spawn(path).await?;
        let _ = self.sync_document(&client, path).await?;
        query_symbol_read(&client, path, symbol_name).await
    }

    async fn find_callers(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<SymbolLocation>, String> {
        let client = self.get_or_spawn(path).await?;
        let _ = self.sync_document(&client, path).await?;
        query_find_callers(&client, path, line, character).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_jsonrpc_lsp_server_handles_diagnostics_symbols_and_callers() {
        let tmp = tempfile::tempdir().unwrap();
        let demo = tmp.path().join("demo.rs");
        fs::write(&demo, "fn alpha() {\n    let x = 1;\n}\n").unwrap();

        let mock_server = tmp.path().join("mock_lsp.py");
        fs::write(
            &mock_server,
            r#"import json, sys

def send(obj):
    raw = json.dumps(obj).encode('utf-8')
    sys.stdout.buffer.write(f"Content-Length: {len(raw)}\r\n\r\n".encode('ascii') + raw)
    sys.stdout.buffer.flush()

while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.decode('ascii').strip()
        if not line:
            break
        k, v = line.split(':', 1)
        headers[k.lower()] = v.strip()
    body = sys.stdin.buffer.read(int(headers['content-length']))
    msg = json.loads(body.decode('utf-8'))
    method = msg.get('method')
    if method == 'initialize':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': {'capabilities': {}}})
    elif method == 'textDocument/didOpen':
        uri = msg['params']['textDocument']['uri']
        send({
            'jsonrpc': '2.0',
            'method': 'textDocument/publishDiagnostics',
            'params': {
                'uri': uri,
                'diagnostics': [{
                    'range': {'start': {'line': 1, 'character': 4}, 'end': {'line': 1, 'character': 5}},
                    'severity': 1,
                    'source': 'mock-lsp',
                    'message': 'type mismatch'
                }]
            }
        })
    elif method == 'textDocument/documentSymbol':
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': [{
            'name': 'alpha',
            'kind': 12,
            'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 2, 'character': 1}}
        }]})
    elif method == 'textDocument/references':
        uri = msg['params']['textDocument']['uri']
        send({'jsonrpc': '2.0', 'id': msg['id'], 'result': [{
            'uri': uri,
            'range': {'start': {'line': 0, 'character': 3}, 'end': {'line': 0, 'character': 8}}
        }]})
"#,
        )
        .unwrap();

        let mut mgr = LazyLspManager::new(tmp.path().to_path_buf());
        let python_bin = if cfg!(windows) { "python" } else { "python3" };
        mgr.override_server(
            "rs",
            python_bin.to_string(),
            vec![mock_server.to_string_lossy().into_owned()],
        );

        let diags = mgr.diagnostics_after_write(&demo).await;
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "type mismatch");

        let sym = mgr.symbol_read(&demo, "alpha").await.unwrap();
        assert_eq!(sym.start_line, 1);
        assert!(sym.snippet.contains("fn alpha()"));

        let callers = mgr.find_callers(&demo, 1, 3).await.unwrap();
        assert_eq!(callers.len(), 1);
    }
}
