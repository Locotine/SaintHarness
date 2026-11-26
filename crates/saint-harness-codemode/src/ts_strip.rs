use std::path::Path;

/// Strips TypeScript type annotations into executable JS for `rquickjs` without Node/Bun.
pub fn strip_typescript_types(path: &Path, source: &str) -> Result<String, String> {
    #[cfg(feature = "ts-plugins")]
    {
        use oxc_allocator::Allocator;
        use oxc_codegen::Codegen;
        use oxc_parser::Parser;
        use oxc_semantic::SemanticBuilder;
        use oxc_span::SourceType;
        use oxc_transformer::{TransformOptions, Transformer};

        let allocator = Allocator::default();
        let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
        let parsed = Parser::new(&allocator, source, source_type).parse();
        if !parsed.diagnostics.is_empty() {
            let errs: Vec<String> = parsed
                .diagnostics
                .into_iter()
                .map(|e| e.to_string())
                .collect();
            return Err(format!("TS parse error: {}", errs.join("; ")));
        }
        let mut program = parsed.program;
        let scoping = SemanticBuilder::new()
            .build(&program)
            .semantic
            .into_scoping();
        let opts = TransformOptions::default();
        let ret =
            Transformer::new(&allocator, path, &opts).build_with_scoping(scoping, &mut program);
        if !ret.diagnostics.is_empty() {
            let errs: Vec<String> = ret.diagnostics.into_iter().map(|e| e.to_string()).collect();
            return Err(format!("TS transform error: {}", errs.join("; ")));
        }
        Ok(Codegen::new().build(&program).code)
    }

    #[cfg(not(feature = "ts-plugins"))]
    {
        let _ = path;
        Ok(strip_basic_ts_fallback(source))
    }
}

#[cfg(not(feature = "ts-plugins"))]
fn strip_basic_ts_fallback(source: &str) -> String {
    // Minimal fallback removing `interface`/`type` declarations and simple `: Type` annotations.
    let mut out = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("interface ") || trimmed.starts_with("type ") {
            continue;
        }
        let cleaned = line
            .replace(": string", "")
            .replace(": number", "")
            .replace(": boolean", "")
            .replace(": any", "")
            .replace(": void", "");
        out.push_str(&cleaned);
        out.push('\n');
    }
    out
}
