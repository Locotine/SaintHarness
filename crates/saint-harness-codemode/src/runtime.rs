use async_trait::async_trait;
use rquickjs::{Context, Function, Runtime};
use saint_harness_schema::{ScriptHost, ScriptResult, ToolInvoker};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_TOOL_CALLS: usize = 50;

/// Embedded `rquickjs` sandbox executing JavaScript on a dedicated OS thread with a 64 MiB memory
/// cap, hard interrupt timer (`Arc<AtomicBool>`), `max_tool_calls` budget, and zero FS/Net access
/// outside the injected `ToolInvoker` bridge.
#[derive(Debug, Clone)]
pub struct QuickJsScriptHost {
    pub memory_limit_bytes: usize,
}

impl Default for QuickJsScriptHost {
    fn default() -> Self {
        Self {
            memory_limit_bytes: DEFAULT_MEMORY_LIMIT_BYTES,
        }
    }
}

#[async_trait]
impl ScriptHost for QuickJsScriptHost {
    async fn execute_script(
        &self,
        code: &str,
        timeout_ms: u64,
        max_tool_calls: usize,
        invoker: Arc<dyn ToolInvoker>,
    ) -> Result<ScriptResult, String> {
        let code = code.to_string();
        let mem_limit = self.memory_limit_bytes;
        let rt_handle = tokio::runtime::Handle::current();

        tokio::task::spawn_blocking(move || {
            run_quickjs_sync(
                &code,
                timeout_ms,
                max_tool_calls,
                mem_limit,
                invoker,
                rt_handle,
            )
        })
        .await
        .map_err(|e| format!("quickjs worker join failed: {e}"))?
    }
}

fn run_quickjs_sync(
    code: &str,
    timeout_ms: u64,
    max_tool_calls: usize,
    memory_limit_bytes: usize,
    invoker: Arc<dyn ToolInvoker>,
    rt_handle: tokio::runtime::Handle,
) -> Result<ScriptResult, String> {
    let runtime = Runtime::new().map_err(|e| format!("quickjs runtime init failed: {e}"))?;
    runtime.set_memory_limit(memory_limit_bytes);
    runtime.set_max_stack_size(1024 * 1024);

    let interrupted = Arc::new(AtomicBool::new(false));
    let intr_check = interrupted.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || intr_check.load(Ordering::Relaxed))));

    let intr_timer = interrupted.clone();
    let timer_done = Arc::new(AtomicBool::new(false));
    let timer_done_bg = timer_done.clone();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms.max(50));
        while std::time::Instant::now() < deadline {
            if timer_done_bg.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        intr_timer.store(true, Ordering::Relaxed);
    });

    let ctx = Context::full(&runtime).map_err(|e| format!("quickjs context init failed: {e}"))?;
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_made = Arc::new(AtomicUsize::new(0));

    let eval_res: Result<String, String> = ctx.with(|ctx| {
        let globals = ctx.globals();

        let logs_cb = logs.clone();
        let log_fn = Function::new(ctx.clone(), move |msg: String| {
            if let Ok(mut guard) = logs_cb.lock() {
                guard.push(msg);
            }
        })
        .map_err(|e| e.to_string())?;
        globals.set("__host_log", log_fn).map_err(|e| e.to_string())?;

        let inv = invoker.clone();
        let counter = calls_made.clone();
        let handle = rt_handle.clone();
        let call_fn = Function::new(
            ctx.clone(),
            move |ns: String, name: String, args_json: String| -> String {
                let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if count > max_tool_calls {
                    return serde_json::json!({
                        "__error": format!("max_tool_calls limit ({max_tool_calls}) exceeded")
                    })
                    .to_string();
                }
                let args: Value = serde_json::from_str(&args_json).unwrap_or(Value::Null);
                let result = handle.block_on(async { inv.invoke(&ns, &name, args).await });
                match result {
                    Ok(v) => serde_json::json!({ "ok": v }).to_string(),
                    Err(e) => serde_json::json!({ "__error": e }).to_string(),
                }
            },
        )
        .map_err(|e| e.to_string())?;
        globals
            .set("__host_call_tool", call_fn)
            .map_err(|e| e.to_string())?;

        let prelude = r#"
const console = { log: (...a) => __host_log(a.map(x => typeof x === 'string' ? x : JSON.stringify(x)).join(' ')) };
const tools = new Proxy({
  call(ns, fn, args) {
    const raw = __host_call_tool(String(ns), String(fn), JSON.stringify(args ?? {}));
    const parsed = JSON.parse(raw);
    if (parsed.__error) throw new Error(parsed.__error);
    return parsed.ok;
  }
}, {
  get(target, ns) {
    if (ns in target) return target[ns];
    return new Proxy({}, {
      get(_, fn) {
        return (args) => target.call(String(ns), String(fn), args ?? {});
      }
    });
  }
});
"#;
        ctx.eval::<(), _>(prelude).map_err(|e| e.to_string())?;

        let wrapped = format!("JSON.stringify((() => {{ {code} }})())");
        let val: rquickjs::Value = ctx.eval(wrapped.as_str()).map_err(|e| {
            if interrupted.load(Ordering::Relaxed) {
                format!("script interrupted after {timeout_ms}ms timeout")
            } else {
                format!("quickjs execution error: {e}")
            }
        })?;
        if let Some(s) = val.as_string() {
            s.to_string().map_err(|e| e.to_string())
        } else {
            Ok("null".to_string())
        }
    });

    timer_done.store(true, Ordering::Relaxed);
    let raw_out = eval_res?;
    let output: Value = serde_json::from_str(&raw_out).unwrap_or(Value::String(raw_out));
    let collected_logs = logs.lock().map(|g| g.clone()).unwrap_or_default();

    Ok(ScriptResult {
        output,
        tool_calls_made: calls_made.load(Ordering::Relaxed),
        logs: collected_logs,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FakeInvoker;
    #[async_trait]
    impl ToolInvoker for FakeInvoker {
        async fn invoke(&self, ns: &str, name: &str, args: Value) -> Result<Value, String> {
            Ok(json!({ "ns": ns, "name": name, "args": args, "items": ["a.rs", "b.rs", "c.txt"] }))
        }
    }

    #[tokio::test]
    async fn executes_js_with_tools_proxy_blocks_infinite_loop_and_oom() {
        let host = QuickJsScriptHost::default();
        let res = host
            .execute_script(
                r#"
                const r = tools.fs.glob({ pattern: "*.rs" });
                console.log("found", r.items.length);
                return r.items.filter(x => x.endsWith(".rs"));
                "#,
                2000,
                50,
                Arc::new(FakeInvoker),
            )
            .await
            .unwrap();
        assert_eq!(res.output, json!(["a.rs", "b.rs"]));
        assert_eq!(res.tool_calls_made, 1);

        // Infinite loop interrupted cleanly by timeout
        let loop_err = host
            .execute_script("while(true) {}", 250, 10, Arc::new(FakeInvoker))
            .await
            .unwrap_err();
        assert!(loop_err.contains("interrupted") || loop_err.contains("execution error"));

        // OOM allocation blocked at 64 MiB cap
        let oom_err = host
            .execute_script(
                "const a = []; for (let i = 0; i < 10000000; i++) { a.push('xxxxxxxxxx'.repeat(100)); } return a.length;",
                3000,
                10,
                Arc::new(FakeInvoker),
            )
            .await;
        assert!(oom_err.is_err());
    }
}
