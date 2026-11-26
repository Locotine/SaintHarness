use crate::grammars::parse_source;
use std::path::Path;
use tree_sitter::Node;

const ALPHABET: &[u8; 16] = b"ZPMQVRWSNKTXJBYH";

fn short_hash(line: &str, prev: Option<&str>, next: Option<&str>) -> String {
    let norm = line.trim();
    let mut h: u32 = 0x811c_9dc5;
    let feed = |state: &mut u32, bytes: &[u8]| {
        for &b in bytes {
            *state ^= u32::from(b);
            *state = state.wrapping_mul(0x0100_0193);
        }
    };
    feed(&mut h, norm.as_bytes());
    if norm.len() <= 2 || !norm.chars().any(char::is_alphanumeric) {
        feed(&mut h, &[0x1f]);
        if let Some(p) = prev {
            feed(&mut h, p.trim().as_bytes());
        }
        feed(&mut h, &[0x1e]);
        if let Some(n) = next {
            feed(&mut h, n.trim().as_bytes());
        }
    }
    let mut s = String::with_capacity(2);
    s.push(ALPHABET[((h >> 4) & 0x0f) as usize] as char);
    s.push(ALPHABET[(h & 0x0f) as usize] as char);
    s
}

/// Produces a compact structural AST outline with line ranges and Hashline anchors.
pub fn outline_source(path: &Path, source: &str) -> Result<Option<String>, String> {
    let Some((_, tree)) = parse_source(path, source) else {
        return Ok(None);
    };
    let lines: Vec<&str> = source.lines().collect();
    let mut items = Vec::new();
    collect_outline_nodes(tree.root_node(), &lines, 0, &mut items);
    if items.is_empty() {
        return Ok(None);
    }
    Ok(Some(items.join("\n")))
}

fn collect_outline_nodes(node: Node<'_>, lines: &[&str], depth: usize, out: &mut Vec<String>) {
    let kind = node.kind();
    let is_item = matches!(
        kind,
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "type_item"
            | "function_declaration"
            | "method_definition"
            | "class_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
            | "function_definition"
            | "class_definition"
            | "method_declaration"
            | "type_declaration"
    );

    if is_item {
        let start_row = node.start_position().row;
        let end_row = node.end_position().row;
        if let Some(sig_line) = lines.get(start_row) {
            let prev = if start_row > 0 {
                lines.get(start_row - 1).copied()
            } else {
                None
            };
            let next = lines.get(start_row + 1).copied();
            let h = short_hash(sig_line, prev, next);
            let indent = "  ".repeat(depth.min(3));
            let trimmed = sig_line.trim().trim_end_matches('{').trim();
            out.push(format!(
                "{}#{h}|{indent}{trimmed}  [L{}-L{}]",
                start_row + 1,
                start_row + 1,
                end_row + 1
            ));
        }
        if matches!(
            kind,
            "impl_item" | "trait_item" | "class_declaration" | "class_definition" | "mod_item"
        ) {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_outline_nodes(child, lines, depth + 1, out);
            }
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_outline_nodes(child, lines, depth, out);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn outline_reduces_large_file_by_over_80_percent() {
        let mut src = String::new();
        for i in 0..10 {
            src.push_str(&format!("pub fn worker_{i}(input: &str) -> usize {{\n"));
            for j in 0..20 {
                src.push_str(&format!("    let step_{j} = input.len() + {j};\n"));
            }
            src.push_str("    input.len()\n}\n\n");
        }
        let outline = outline_source(&PathBuf::from("big.rs"), &src)
            .unwrap()
            .unwrap();
        assert!(outline.contains("pub fn worker_0"));
        assert!(outline.contains("pub fn worker_9"));
        assert!(
            outline.len() * 5 < src.len(),
            "outline must compress >= 80%"
        );
    }
}
