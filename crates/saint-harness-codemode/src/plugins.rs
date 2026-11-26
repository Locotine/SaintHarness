use crate::runtime::QuickJsScriptHost;
use crate::ts_strip::strip_typescript_types;
use async_trait::async_trait;
use saint_harness_schema::{ScriptHost, ToolInvoker};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedPlugin {
    pub name: String,
    pub path: PathBuf,
    pub js_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginHookVerdict {
    Allow,
    Deny { reason: String },
}

#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<LoadedPlugin>,
    host: QuickJsScriptHost,
}

struct NoopInvoker;
#[async_trait]
impl ToolInvoker for NoopInvoker {
    async fn invoke(&self, _ns: &str, _name: &str, _args: Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
}

impl PluginRegistry {
    pub fn discover_dirs(dirs: &[PathBuf]) -> Self {
        let mut reg = Self::default();
        for dir in dirs {
            reg.load_dir(dir);
        }
        reg
    }

    pub fn load_dir(&mut self, dir: &Path) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "js" && ext != "ts" {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let js_source = if ext == "ts" {
                match strip_typescript_types(&path, &raw) {
                    Ok(js) => js,
                    Err(_) => continue,
                }
            } else {
                raw
            };
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("plugin")
                .to_string();
            self.plugins.push(LoadedPlugin {
                name,
                path,
                js_source,
            });
        }
    }

    pub fn list(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    pub async fn run_tool_before_hooks(&self, tool: &str, args: &Value) -> PluginHookVerdict {
        for plugin in &self.plugins {
            let payload = json!({ "hook": "tool.execute.before", "tool": tool, "args": args });
            let script = format!(
                "{}\nif (typeof onToolBefore === 'function') {{ return onToolBefore({}); }}\nreturn {{ allow: true }};",
                plugin.js_source, payload
            );
            if let Ok(res) = self
                .host
                .execute_script(&script, 1000, 0, Arc::new(NoopInvoker))
                .await
                && res.output.get("allow").and_then(|v| v.as_bool()) == Some(false)
            {
                let reason = res
                    .output
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("blocked by plugin")
                    .to_string();
                return PluginHookVerdict::Deny { reason };
            }
        }
        PluginHookVerdict::Allow
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn js_and_ts_guard_plugins_block_dangerous_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_file = tmp.path().join("guard.ts");
        fs::write(
            &plugin_file,
            r#"
function onToolBefore(ev: any): any {
  const cmd: string = (ev.args && ev.args.command) || "";
  if (cmd.indexOf("rm -rf /") !== -1) {
    return { allow: false, reason: "destructive root delete blocked by guard plugin" };
  }
  return { allow: true };
}
"#,
        )
        .unwrap();

        let reg = PluginRegistry::discover_dirs(&[tmp.path().to_path_buf()]);
        assert_eq!(reg.list().len(), 1);
        let verdict = reg
            .run_tool_before_hooks("shell", &json!({ "command": "rm -rf /" }))
            .await;
        assert!(matches!(verdict, PluginHookVerdict::Deny { .. }));

        let ok = reg
            .run_tool_before_hooks("shell", &json!({ "command": "cargo test" }))
            .await;
        assert_eq!(ok, PluginHookVerdict::Allow);
    }
}
