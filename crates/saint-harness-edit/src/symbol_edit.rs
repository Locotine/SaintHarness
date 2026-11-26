use saint_harness_schema::{EditDraft, EditRequest, EditStrategy};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceSymbolPayload {
    pub symbol_name: String,
    pub replacement: String,
}

pub fn replace_symbol_in_source(
    path: &Path,
    source: &str,
    symbol_name: &str,
    replacement: &str,
) -> Result<String, String> {
    #[cfg(feature = "ast-edit")]
    if let Some((start_byte, end_byte)) = find_symbol_ast_span(path, source, symbol_name) {
        let mut out = String::with_capacity(source.len() + replacement.len());
        out.push_str(&source[..start_byte]);
        out.push_str(replacement);
        out.push_str(&source[end_byte..]);
        return Ok(out);
    }

    // Fallback: line-anchored brace matcher when language grammar isn't available.
    let _ = path;
    find_and_replace_fallback(source, symbol_name, replacement)
}

#[cfg(feature = "ast-edit")]
fn find_symbol_ast_span(path: &Path, source: &str, symbol_name: &str) -> Option<(usize, usize)> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    let lang: tree_sitter::Language = match ext {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "js" | "jsx" => tree_sitter_javascript::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        _ => return None,
    };
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang).ok()?;
    let tree = parser.parse(source, None)?;
    let bytes = source.as_bytes();
    find_named_decl(tree.root_node(), bytes, symbol_name)
}

#[cfg(feature = "ast-edit")]
fn find_named_decl(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    symbol_name: &str,
) -> Option<(usize, usize)> {
    let kind = node.kind();
    let is_decl = matches!(
        kind,
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "function_declaration"
            | "method_definition"
            | "class_declaration"
            | "interface_declaration"
            | "function_definition"
            | "class_definition"
            | "method_declaration"
            | "type_declaration"
    );
    if is_decl
        && let Some(name_node) = node.child_by_field_name("name")
        && name_node.utf8_text(source).ok() == Some(symbol_name)
    {
        return Some((node.start_byte(), node.end_byte()));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_named_decl(child, source, symbol_name) {
            return Some(found);
        }
    }
    None
}

fn find_and_replace_fallback(
    source: &str,
    symbol_name: &str,
    replacement: &str,
) -> Result<String, String> {
    let patterns = [
        format!("fn {symbol_name}"),
        format!("struct {symbol_name}"),
        format!("function {symbol_name}"),
        format!("def {symbol_name}"),
        format!("func {symbol_name}"),
    ];
    let start_byte = patterns
        .iter()
        .filter_map(|p| source.find(p))
        .min()
        .ok_or_else(|| format!("symbol '{symbol_name}' not found"))?;
    let line_start = source[..start_byte].rfind('\n').map(|i| i + 1).unwrap_or(0);

    let mut depth = 0i32;
    let mut saw_brace = false;
    let mut end_byte = source.len();
    for (rel, ch) in source[line_start..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                saw_brace = true;
            }
            '}' => {
                depth -= 1;
                if saw_brace && depth == 0 {
                    end_byte = line_start + rel + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    let mut out = String::with_capacity(source.len() + replacement.len());
    out.push_str(&source[..line_start]);
    out.push_str(replacement);
    out.push_str(&source[end_byte..]);
    Ok(out)
}

pub struct ReplaceSymbolStrategy;

impl EditStrategy for ReplaceSymbolStrategy {
    fn name(&self) -> &str {
        "replace_symbol"
    }

    fn compute_draft(&self, original: &str, request: &EditRequest) -> Result<EditDraft, String> {
        let payload: ReplaceSymbolPayload = serde_json::from_value(request.payload.clone())
            .map_err(|e| format!("invalid replace_symbol payload: {e}"))?;
        let updated = replace_symbol_in_source(
            &request.path,
            original,
            &payload.symbol_name,
            &payload.replacement,
        )?;
        Ok(EditDraft {
            path: request.path.clone(),
            original_text: original.to_string(),
            updated_text: updated,
            strategy_name: self.name().to_string(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn replaces_rust_function_by_ast_range() {
        let src = "pub fn keep() -> i32 { 1 }\n\npub fn target(x: i32) -> i32 {\n    x + 1\n}\n";
        let updated = replace_symbol_in_source(
            &PathBuf::from("sample.rs"),
            src,
            "target",
            "pub fn target(x: i32) -> i32 {\n    x * 2\n}",
        )
        .unwrap();
        assert!(updated.contains("pub fn keep() -> i32 { 1 }"));
        assert!(updated.contains("x * 2"));
        assert!(!updated.contains("x + 1"));
    }
}
