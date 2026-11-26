use std::path::Path;
use tree_sitter::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageId {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Json,
    Toml,
}

impl LanguageId {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "rs" => Some(Self::Rust),
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "py" => Some(Self::Python),
            "go" => Some(Self::Go),
            "json" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            _ => None,
        }
    }

    #[cfg(feature = "lang-core")]
    pub fn ts_language(self) -> Language {
        match self {
            LanguageId::Rust => tree_sitter_rust::LANGUAGE.into(),
            LanguageId::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            LanguageId::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            LanguageId::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            LanguageId::Python => tree_sitter_python::LANGUAGE.into(),
            LanguageId::Go => tree_sitter_go::LANGUAGE.into(),
            LanguageId::Json => tree_sitter_json::LANGUAGE.into(),
            LanguageId::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
        }
    }

    #[cfg(not(feature = "lang-core"))]
    pub fn ts_language(self) -> Option<Language> {
        let _ = self;
        None
    }
}

pub fn parse_source(path: &Path, source: &str) -> Option<(LanguageId, tree_sitter::Tree)> {
    let lang_id = LanguageId::from_path(path)?;
    #[cfg(feature = "lang-core")]
    {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang_id.ts_language()).ok()?;
        let tree = parser.parse(source, None)?;
        Some((lang_id, tree))
    }
    #[cfg(not(feature = "lang-core"))]
    {
        let _ = (lang_id, source);
        None
    }
}
