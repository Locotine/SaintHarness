use async_trait::async_trait;
use ignore::WalkBuilder;
use saint_harness_schema::{IndexHit, MonorepoIndex};
#[cfg(not(feature = "monorepo-index"))]
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(feature = "monorepo-index"))]
use std::sync::RwLock;

#[cfg(feature = "monorepo-index")]
mod tantivy_impl {
    use super::*;
    use tantivy::collector::TopDocs;
    use tantivy::query::QueryParser;
    use tantivy::schema::{STORED, Schema, TEXT, Value};
    use tantivy::{Index, IndexWriter, Term, doc};

    pub struct TantivyEngine {
        index: Index,
        path_field: tantivy::schema::Field,
        body_field: tantivy::schema::Field,
        writer: std::sync::Mutex<IndexWriter>,
    }

    impl TantivyEngine {
        pub fn open(index_dir: &Path) -> Result<Self, String> {
            fs::create_dir_all(index_dir).map_err(|e| e.to_string())?;
            let mut builder = Schema::builder();
            let path_field = builder.add_text_field("path", tantivy::schema::STRING | STORED);
            let body_field = builder.add_text_field("body", TEXT | STORED);
            let schema = builder.build();
            let index = Index::open_in_dir(index_dir)
                .or_else(|_| Index::create_in_dir(index_dir, schema.clone()))
                .map_err(|e| e.to_string())?;
            let writer = index.writer(32 * 1024 * 1024).map_err(|e| e.to_string())?;
            Ok(Self {
                index,
                path_field,
                body_field,
                writer: std::sync::Mutex::new(writer),
            })
        }

        pub fn index_all(&self, workspace_root: &Path) -> Result<usize, String> {
            let mut writer = self.writer.lock().map_err(|e| e.to_string())?;
            let _ = writer.delete_all_documents();
            let mut count = 0usize;
            for entry in WalkBuilder::new(workspace_root)
                .hidden(true)
                .build()
                .flatten()
            {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let Ok(body) = fs::read_to_string(p) else {
                    continue;
                };
                let rel = p
                    .strip_prefix(workspace_root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/");
                let _ = writer.add_document(doc!(
                    self.path_field => rel,
                    self.body_field => body,
                ));
                count += 1;
            }
            writer.commit().map_err(|e| e.to_string())?;
            Ok(count)
        }

        pub fn upsert(&self, workspace_root: &Path, rel_path: &str) -> Result<(), String> {
            let mut writer = self.writer.lock().map_err(|e| e.to_string())?;
            writer.delete_term(Term::from_field_text(self.path_field, rel_path));
            let abs = workspace_root.join(rel_path);
            if let Ok(body) = fs::read_to_string(abs) {
                let _ = writer.add_document(doc!(
                    self.path_field => rel_path.to_string(),
                    self.body_field => body,
                ));
            }
            writer.commit().map_err(|e| e.to_string())?;
            Ok(())
        }

        pub fn search(&self, query: &str, limit: usize) -> Result<Vec<IndexHit>, String> {
            let reader = self.index.reader().map_err(|e| e.to_string())?;
            let searcher = reader.searcher();
            let qp = QueryParser::for_index(&self.index, vec![self.body_field, self.path_field]);
            let parsed = qp.parse_query(query).map_err(|e| e.to_string())?;
            let docs = searcher
                .search(&parsed, &TopDocs::with_limit(limit))
                .map_err(|e| e.to_string())?;
            let mut hits = Vec::new();
            for (score, addr) in docs {
                if let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(addr) {
                    let path = doc
                        .get_first(self.path_field)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let snippet = doc
                        .get_first(self.body_field)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .lines()
                        .take(2)
                        .collect::<Vec<_>>()
                        .join(" ");
                    hits.push(IndexHit {
                        path,
                        score,
                        snippet,
                    });
                }
            }
            Ok(hits)
        }
    }
}

pub struct WorkspaceBm25Index {
    #[cfg(feature = "monorepo-index")]
    engine: tantivy_impl::TantivyEngine,
    #[cfg(not(feature = "monorepo-index"))]
    docs: RwLock<HashMap<String, String>>,
    _index_dir: PathBuf,
}

impl WorkspaceBm25Index {
    pub fn open(index_dir: PathBuf) -> Result<Self, String> {
        #[cfg(feature = "monorepo-index")]
        {
            let engine = tantivy_impl::TantivyEngine::open(&index_dir)?;
            Ok(Self {
                engine,
                _index_dir: index_dir,
            })
        }
        #[cfg(not(feature = "monorepo-index"))]
        {
            let _ = fs::create_dir_all(&index_dir);
            Ok(Self {
                docs: RwLock::new(HashMap::new()),
                _index_dir: index_dir,
            })
        }
    }
}

#[async_trait]
impl MonorepoIndex for WorkspaceBm25Index {
    async fn index_workspace(&self, workspace_root: &Path) -> Result<usize, String> {
        #[cfg(feature = "monorepo-index")]
        {
            self.engine.index_all(workspace_root)
        }
        #[cfg(not(feature = "monorepo-index"))]
        {
            let mut guard = self.docs.write().map_err(|e| e.to_string())?;
            guard.clear();
            for entry in WalkBuilder::new(workspace_root)
                .hidden(true)
                .build()
                .flatten()
            {
                let p = entry.path();
                if p.is_file()
                    && let Ok(body) = fs::read_to_string(p)
                {
                    let rel = p
                        .strip_prefix(workspace_root)
                        .unwrap_or(p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    guard.insert(rel, body);
                }
            }
            Ok(guard.len())
        }
    }

    async fn update_file(&self, workspace_root: &Path, rel_path: &str) -> Result<(), String> {
        #[cfg(feature = "monorepo-index")]
        {
            self.engine.upsert(workspace_root, rel_path)
        }
        #[cfg(not(feature = "monorepo-index"))]
        {
            let mut guard = self.docs.write().map_err(|e| e.to_string())?;
            let abs = workspace_root.join(rel_path);
            if let Ok(body) = fs::read_to_string(abs) {
                guard.insert(rel_path.to_string(), body);
            } else {
                guard.remove(rel_path);
            }
            Ok(())
        }
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<IndexHit>, String> {
        #[cfg(feature = "monorepo-index")]
        {
            self.engine.search(query, limit)
        }
        #[cfg(not(feature = "monorepo-index"))]
        {
            let terms: Vec<String> = query
                .split_whitespace()
                .map(|t| t.to_ascii_lowercase())
                .collect();
            let guard = self.docs.read().map_err(|e| e.to_string())?;
            let mut scored: Vec<IndexHit> = Vec::new();
            for (path, body) in guard.iter() {
                let lower = body.to_ascii_lowercase();
                let mut score = 0.0_f32;
                for term in &terms {
                    let count = lower.matches(term.as_str()).count();
                    score += count as f32;
                }
                if score > 0.0 {
                    let snippet = body.lines().take(2).collect::<Vec<_>>().join(" ");
                    scored.push(IndexHit {
                        path: path.clone(),
                        score,
                        snippet,
                    });
                }
            }
            scored.sort_by(|a, b| b.score.total_cmp(&a.score));
            scored.truncate(limit);
            Ok(scored)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn indexes_searches_and_incrementally_updates_files() {
        let ws = tempfile::tempdir().unwrap();
        let idx_dir = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("alpha.rs"), "pub fn wal_writer_commit() {}").unwrap();
        fs::write(ws.path().join("beta.rs"), "pub fn unrelated_helper() {}").unwrap();

        let idx = WorkspaceBm25Index::open(idx_dir.path().to_path_buf()).unwrap();
        assert_eq!(idx.index_workspace(ws.path()).await.unwrap(), 2);

        let hits = idx.search("wal_writer_commit", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "alpha.rs");

        fs::write(
            ws.path().join("beta.rs"),
            "pub fn unique_incremental_token() {}",
        )
        .unwrap();
        idx.update_file(ws.path(), "beta.rs").await.unwrap();
        let hits2 = idx.search("unique_incremental_token", 5).await.unwrap();
        assert_eq!(hits2.len(), 1);
        assert_eq!(hits2[0].path, "beta.rs");
    }
}
