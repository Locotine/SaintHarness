use crate::client::JsonRpcLspClient;
use saint_harness_schema::DiagnosticItem;
use std::path::Path;
use std::time::Duration;

pub const DEBOUNCE_MS: u64 = 150;
pub const DIAGNOSTICS_CAP_MS: u64 = 1_500;
pub const MAX_ERRORS: usize = 15;

pub async fn collect_error_diagnostics(
    client: &JsonRpcLspClient,
    path: &Path,
    uri: &str,
) -> Vec<DiagnosticItem> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(DIAGNOSTICS_CAP_MS);
    tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)).await;

    let mut raw = client.cached_diagnostics(uri);
    while raw.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
        raw = client.cached_diagnostics(uri);
    }

    let path_str = path.to_string_lossy().into_owned();
    raw.into_iter()
        .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
        .take(MAX_ERRORS)
        .map(|d| DiagnosticItem {
            path: path_str.clone(),
            line: d.range.start.line + 1,
            column: d.range.start.character + 1,
            severity: "error".to_string(),
            message: d.message,
            source: d.source.unwrap_or_else(|| "lsp".to_string()),
        })
        .collect()
}
