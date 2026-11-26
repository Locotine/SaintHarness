use saint_harness_schema::{EditDraft, EditRequest, EditStrategy};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstEditPayload {
    pub pattern: String,
    pub rewrite: String,
}

#[cfg(feature = "ast-edit")]
#[derive(Clone, Copy)]
enum SupportedLang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
}

#[cfg(feature = "ast-edit")]
impl SupportedLang {
    fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|e| e.to_str())? {
            "rs" => Some(Self::Rust),
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "jsx" => Some(Self::JavaScript),
            "py" => Some(Self::Python),
            "go" => Some(Self::Go),
            _ => None,
        }
    }
}

#[cfg(feature = "ast-edit")]
impl ast_grep_core::Language for SupportedLang {
    fn get_ts_language(&self) -> ast_grep_core::language::TSLanguage {
        match self {
            SupportedLang::Rust => tree_sitter_rust::LANGUAGE.into(),
            SupportedLang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            SupportedLang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            SupportedLang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            SupportedLang::Python => tree_sitter_python::LANGUAGE.into(),
            SupportedLang::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    fn expando_char(&self) -> char {
        match self {
            SupportedLang::Rust | SupportedLang::Python | SupportedLang::Go => 'µ',
            SupportedLang::TypeScript | SupportedLang::Tsx | SupportedLang::JavaScript => '$',
        }
    }

    fn pre_process_pattern<'q>(&self, query: &'q str) -> std::borrow::Cow<'q, str> {
        let exp = self.expando_char();
        if exp == '$' || !query.contains('$') {
            std::borrow::Cow::Borrowed(query)
        } else {
            let mut s = String::with_capacity(query.len() + 8);
            for ch in query.chars() {
                if ch == '$' {
                    s.push(exp);
                } else {
                    s.push(ch);
                }
            }
            std::borrow::Cow::Owned(s)
        }
    }
}

/// Applies structural AST search & rewrite using `ast-grep-core` so comments and string literals
/// with colliding text are never touched.
pub fn apply_ast_rewrite(
    path: &Path,
    source: &str,
    pattern: &str,
    rewrite: &str,
) -> Result<(String, usize), String> {
    #[cfg(feature = "ast-edit")]
    {
        use ast_grep_core::{AstGrep, Language};

        let lang = SupportedLang::from_path(path)
            .ok_or_else(|| format!("ast_edit unsupported extension for {}", path.display()))?;
        let grep = AstGrep::new(source, lang);
        let matcher = match ast_grep_core::Pattern::try_new(pattern, lang) {
            Ok(p) if !p.has_error() => p,
            _ if matches!(lang, SupportedLang::Rust) => {
                // Rust grammar requires expressions to sit inside a function body to parse without ERROR nodes.
                let wrapped = format!("fn __sg() {{ {} }}", lang.pre_process_pattern(pattern));
                let ctx_grep = AstGrep::new(&wrapped, lang);
                let block_expr = ctx_grep
                    .root()
                    .child(0)
                    .and_then(|f| f.field("body"))
                    .and_then(|b| b.child(1));
                match block_expr {
                    Some(expr) => ast_grep_core::Pattern::from(expr),
                    None => ast_grep_core::Pattern::try_new(pattern, lang)
                        .map_err(|e| format!("invalid ast_edit pattern: {e}"))?,
                }
            }
            Err(e) => return Err(format!("invalid ast_edit pattern: {e}")),
            Ok(p) => p,
        };

        let mut edits = Vec::new();
        for m in grep.root().find_all(&matcher) {
            edits.push(m.replace_by(rewrite));
        }
        let count = edits.len();
        if count == 0 {
            return Ok((source.to_string(), 0));
        }

        // Apply bottom-up by descending byte offset.
        edits.sort_by_key(|a| std::cmp::Reverse(a.position));
        let mut bytes = source.as_bytes().to_vec();
        for edit in edits {
            let end = edit
                .position
                .saturating_add(edit.deleted_length)
                .min(bytes.len());
            bytes.splice(edit.position..end, edit.inserted_text);
        }
        let updated = String::from_utf8(bytes)
            .map_err(|e| format!("ast_edit produced non-UTF8 output: {e}"))?;
        Ok((updated, count))
    }

    #[cfg(not(feature = "ast-edit"))]
    {
        let _ = (path, source, pattern, rewrite);
        Err("ast-edit feature is disabled in this build".to_string())
    }
}

pub struct AstEditStrategy;

impl EditStrategy for AstEditStrategy {
    fn name(&self) -> &str {
        "ast_edit"
    }

    fn compute_draft(&self, original: &str, request: &EditRequest) -> Result<EditDraft, String> {
        let payload: AstEditPayload = serde_json::from_value(request.payload.clone())
            .map_err(|e| format!("invalid ast_edit payload: {e}"))?;
        let (updated, count) =
            apply_ast_rewrite(&request.path, original, &payload.pattern, &payload.rewrite)?;
        if count == 0 {
            return Err(format!(
                "ast_edit matched 0 AST nodes for pattern `{}`",
                payload.pattern
            ));
        }
        Ok(EditDraft {
            path: request.path.clone(),
            original_text: original.to_string(),
            updated_text: updated,
            strategy_name: self.name().to_string(),
        })
    }
}

#[cfg(all(test, feature = "ast-edit"))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ast_edit_rewrites_only_ast_nodes_and_ignores_comments_and_strings() {
        let rust_src = r#"
// Comment mentioning obj.old_call(1, 2) must stay untouched
fn demo(obj: &Svc) {
    let doc = "obj.old_call(1, 2)";
    obj.old_call(1, 2);
}
"#;
        let (updated, count) = apply_ast_rewrite(
            &PathBuf::from("demo.rs"),
            rust_src,
            "$OBJ.old_call($$$ARGS)",
            "$OBJ.new_call($$$ARGS)",
        )
        .unwrap();
        assert_eq!(count, 1);
        assert!(updated.contains("// Comment mentioning obj.old_call(1, 2)"));
        assert!(updated.contains("\"obj.old_call(1, 2)\""));
        assert!(updated.contains("obj.new_call(1, 2);"));

        let ts_src = r#"
// client.oldApi(a, b)
const label = "client.oldApi(a, b)";
client.oldApi(a, b);
"#;
        let (ts_out, ts_count) = apply_ast_rewrite(
            &PathBuf::from("demo.ts"),
            ts_src,
            "$OBJ.oldApi($$$ARGS)",
            "$OBJ.newApi($$$ARGS)",
        )
        .unwrap();
        assert_eq!(ts_count, 1);
        assert!(ts_out.contains("// client.oldApi(a, b)"));
        assert!(ts_out.contains("\"client.oldApi(a, b)\""));
        assert!(ts_out.contains("client.newApi(a, b);"));
    }
}
