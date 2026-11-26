use crate::grammars::parse_source;
use ignore::WalkBuilder;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::Node;

#[derive(Debug, Clone)]
struct FileSymbols {
    rel_path: String,
    defs: Vec<(String, usize, String)>,
    refs: Vec<String>,
}

/// Builds a structural repository map ranked by Personalized PageRank over symbol definition/reference
/// graphs, binary-searching the top ranked symbols to fit within `budget_tokens`.
pub fn generate_repo_map(
    workspace_root: &Path,
    budget_tokens: usize,
    focus_files: &[PathBuf],
) -> Result<String, String> {
    let mut files: Vec<FileSymbols> = Vec::new();
    let walker = WalkBuilder::new(workspace_root)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(8))
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.len() > 512 * 1024 {
            continue;
        }
        let Ok(source) = fs::read_to_string(path) else {
            continue;
        };
        let Some((_, tree)) = parse_source(path, &source) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let lines: Vec<&str> = source.lines().collect();
        let mut defs = Vec::new();
        let mut refs = Vec::new();
        extract_defs_and_refs(
            tree.root_node(),
            source.as_bytes(),
            &lines,
            &mut defs,
            &mut refs,
        );
        if !defs.is_empty() || !refs.is_empty() {
            files.push(FileSymbols {
                rel_path: rel,
                defs,
                refs,
            });
        }
    }

    if files.is_empty() {
        return Ok("(no supported source files found for repo_map)".to_string());
    }

    // Build definition index: symbol -> defining file indices
    let mut def_index: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, f) in files.iter().enumerate() {
        for (sym, _, _) in &f.defs {
            def_index.entry(sym.clone()).or_default().push(idx);
        }
    }

    // Directed graph: referencing file -> defining file
    let n = files.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (src_idx, f) in files.iter().enumerate() {
        let mut seen = HashSet::new();
        for r in &f.refs {
            if let Some(targets) = def_index.get(r) {
                for &dst_idx in targets {
                    if dst_idx != src_idx && seen.insert(dst_idx) {
                        adj[src_idx].push(dst_idx);
                    }
                }
            }
        }
    }

    // Personalization vector favors explicitly focused files.
    let focus_set: HashSet<String> = focus_files
        .iter()
        .map(|p| {
            p.strip_prefix(workspace_root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    let mut personalization = vec![1.0_f64; n];
    if !focus_set.is_empty() {
        for (i, f) in files.iter().enumerate() {
            if focus_set.contains(&f.rel_path) {
                personalization[i] = 5.0;
            }
        }
    }
    let p_sum: f64 = personalization.iter().sum();
    for p in &mut personalization {
        *p /= p_sum.max(1e-9);
    }

    // Personalized PageRank (20 power iterations, damping 0.85)
    let mut rank = personalization.clone();
    let damping = 0.85_f64;
    for _ in 0..20 {
        let mut next = vec![0.0_f64; n];
        let mut dangling = 0.0_f64;
        for (i, neighbors) in adj.iter().enumerate() {
            if neighbors.is_empty() {
                dangling += rank[i];
            } else {
                let share = damping * rank[i] / (neighbors.len() as f64);
                for &dst in neighbors {
                    next[dst] += share;
                }
            }
        }
        for i in 0..n {
            next[i] += (1.0 - damping + damping * dangling) * personalization[i];
        }
        rank = next;
    }

    // Rank individual symbols by parent file PageRank + inbound reference frequency.
    let mut sym_ref_counts: HashMap<&str, usize> = HashMap::new();
    for f in &files {
        for r in &f.refs {
            *sym_ref_counts.entry(r.as_str()).or_insert(0) += 1;
        }
    }
    let mut ranked_symbols: Vec<(f64, usize, usize)> = Vec::new();
    for (fi, f) in files.iter().enumerate() {
        for (di, (name, _, _)) in f.defs.iter().enumerate() {
            let ref_boost = 1.0 + (*sym_ref_counts.get(name.as_str()).unwrap_or(&0) as f64).ln_1p();
            ranked_symbols.push((rank[fi] * ref_boost, fi, di));
        }
    }
    ranked_symbols.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| files[a.1].rel_path.cmp(&files[b.1].rel_path))
            .then_with(|| a.2.cmp(&b.2))
    });

    // Binary search maximum count k whose rendered text fits in `budget_tokens` (~4 chars/token).
    let max_chars = budget_tokens.saturating_mul(4).max(200);
    let mut lo = 0usize;
    let mut hi = ranked_symbols.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let rendered = render_top_k(&files, &ranked_symbols[..mid]);
        if rendered.len() <= max_chars {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    Ok(render_top_k(
        &files,
        &ranked_symbols[..lo.max(1).min(ranked_symbols.len())],
    ))
}

fn render_top_k(files: &[FileSymbols], items: &[(f64, usize, usize)]) -> String {
    let mut grouped: BTreeMap<&str, Vec<(usize, &str)>> = BTreeMap::new();
    for &(_, fi, di) in items {
        let f = &files[fi];
        let (_, line_no, ref sig) = f.defs[di];
        grouped
            .entry(f.rel_path.as_str())
            .or_default()
            .push((line_no, sig.as_str()));
    }
    let mut out = String::new();
    for (path, mut syms) in grouped {
        syms.sort_by_key(|(ln, _)| *ln);
        syms.dedup_by_key(|(ln, _)| *ln);
        out.push_str(path);
        out.push_str(":\n");
        for (ln, sig) in syms {
            out.push_str(&format!("  L{ln}: {sig}\n"));
        }
    }
    out
}

fn extract_defs_and_refs(
    node: Node<'_>,
    source: &[u8],
    lines: &[&str],
    defs: &mut Vec<(String, usize, String)>,
    refs: &mut Vec<String>,
) {
    let kind = node.kind();
    let is_def = matches!(
        kind,
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "function_declaration"
            | "class_declaration"
            | "interface_declaration"
            | "function_definition"
            | "class_definition"
            | "method_declaration"
            | "type_spec"
    );
    if is_def
        && let Some(name_node) = node.child_by_field_name("name")
        && let Ok(name) = name_node.utf8_text(source)
    {
        let row = node.start_position().row;
        let sig = lines
            .get(row)
            .map(|l| l.trim().trim_end_matches('{').trim().to_string())
            .unwrap_or_else(|| name.to_string());
        defs.push((name.to_string(), row + 1, sig));
    } else if matches!(kind, "identifier" | "type_identifier" | "field_identifier")
        && let Ok(ident) = node.utf8_text(source)
        && ident.len() >= 3
    {
        refs.push(ident.to_string());
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_defs_and_refs(child, source, lines, defs, refs);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn repomap_ranks_central_hub_file_first_within_budget() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("core_hub.rs"),
            "pub struct CentralEngine {}\npub fn run_central_engine() -> CentralEngine { CentralEngine {} }\n",
        )
        .unwrap();
        for i in 0..5 {
            fs::write(
                tmp.path().join(format!("caller_{i}.rs")),
                format!(
                    "pub fn use_hub_{i}() {{ let _e = run_central_engine(); let _c = CentralEngine {{}}; }}\n"
                ),
            )
            .unwrap();
        }
        fs::write(tmp.path().join("leaf.rs"), "pub fn unused_leaf() {}\n").unwrap();

        let map = generate_repo_map(tmp.path(), 1500, &[]).unwrap();
        assert!(map.contains("core_hub.rs:"));
        assert!(map.contains("CentralEngine"));
        assert!(map.len() <= 1500 * 4);
    }
}
