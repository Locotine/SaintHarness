use crate::client::JsonRpcLspClient;
use saint_harness_schema::SymbolLocation;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::Duration;
use url::Url;

pub fn file_to_uri(path: &Path) -> Result<String, String> {
    let abs = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Url::from_file_path(&abs)
        .map(|u| u.to_string())
        .map_err(|_| format!("failed to convert {} to file:// URI", abs.display()))
}

pub async fn query_symbol_read(
    client: &JsonRpcLspClient,
    path: &Path,
    symbol_name: &str,
) -> Result<SymbolLocation, String> {
    let uri = file_to_uri(path)?;
    let res = client
        .request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
            Duration::from_millis(1500),
        )
        .await?;

    let source = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = source.lines().collect();

    if let Some((kind, start_line, end_line)) = find_symbol_in_json(&res, symbol_name) {
        let snippet = lines
            .get((start_line as usize)..=(end_line as usize).min(lines.len().saturating_sub(1)))
            .map(|slice| slice.join("\n"))
            .unwrap_or_default();
        return Ok(SymbolLocation {
            name: symbol_name.to_string(),
            kind,
            path: path.to_string_lossy().into_owned(),
            start_line: start_line + 1,
            end_line: end_line + 1,
            snippet,
        });
    }
    Err(format!(
        "symbol '{symbol_name}' not found via LSP in {}",
        path.display()
    ))
}

pub async fn query_find_callers(
    client: &JsonRpcLspClient,
    path: &Path,
    line: u32,
    character: u32,
) -> Result<Vec<SymbolLocation>, String> {
    let uri = file_to_uri(path)?;
    let res = client
        .request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": line.saturating_sub(1), "character": character },
                "context": { "includeDeclaration": false }
            }),
            Duration::from_millis(1500),
        )
        .await?;

    let mut out = Vec::new();
    if let Some(arr) = res.as_array() {
        for loc in arr {
            let loc_uri = loc.get("uri").and_then(|v| v.as_str()).unwrap_or_default();
            let start_line = loc
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let end_line = loc
                .get("range")
                .and_then(|r| r.get("end"))
                .and_then(|s| s.get("line"))
                .and_then(|v| v.as_u64())
                .unwrap_or(start_line as u64) as u32;
            let file_path = Url::parse(loc_uri)
                .ok()
                .and_then(|u| u.to_file_path().ok())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| loc_uri.to_string());
            let snippet = fs::read_to_string(&file_path)
                .ok()
                .and_then(|s| s.lines().nth(start_line as usize).map(ToString::to_string))
                .unwrap_or_default();
            out.push(SymbolLocation {
                name: "reference".to_string(),
                kind: "reference".to_string(),
                path: file_path,
                start_line: start_line + 1,
                end_line: end_line + 1,
                snippet,
            });
        }
    }
    Ok(out)
}

fn find_symbol_in_json(val: &Value, target: &str) -> Option<(String, u32, u32)> {
    let arr = val.as_array()?;
    for item in arr {
        if item.get("name").and_then(|v| v.as_str()) == Some(target) {
            let range = item
                .get("range")
                .or_else(|| item.get("location").and_then(|l| l.get("range")));
            let start = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let end = range
                .and_then(|r| r.get("end"))
                .and_then(|s| s.get("line"))
                .and_then(|v| v.as_u64())
                .unwrap_or(start as u64) as u32;
            let kind = item
                .get("kind")
                .map(ToString::to_string)
                .unwrap_or_else(|| "symbol".to_string());
            return Some((kind, start, end));
        }
        if let Some(children) = item.get("children")
            && let Some(found) = find_symbol_in_json(children, target)
        {
            return Some(found);
        }
    }
    None
}
