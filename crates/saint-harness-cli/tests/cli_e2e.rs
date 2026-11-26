#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
#[cfg(feature = "embedded-server")]
use std::io::Write;
use std::process::Command;
#[cfg(feature = "embedded-server")]
use std::process::Stdio;

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_saint-harness")
}

#[test]
fn cli_version_and_offline_server_status() {
    let ver = Command::new(bin_path()).arg("--version").output().unwrap();
    assert!(ver.status.success());
    let stdout = String::from_utf8_lossy(&ver.stdout);
    assert!(stdout.contains("saint-harness"));

    let offline = Command::new(bin_path())
        .args(["server", "status", "--url", "http://127.0.0.1:1", "--json"])
        .output()
        .unwrap();
    assert_eq!(offline.status.code(), Some(1));
    let body: Value = serde_json::from_slice(&offline.stdout).unwrap();
    assert_eq!(body["reachable"], Value::Bool(false));
    assert_eq!(body["ready"], Value::Bool(false));
}

#[cfg(feature = "embedded-server")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_auth_settings_json_live_run_eval_and_server_status_e2e() {
    use axum::http::HeaderMap;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("sh-home");
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("greeting.txt"), "hello-from-saint-harness-live\n").unwrap();

    // Spin up a real local OpenAI-compatible HTTP + SSE mock LLM endpoint that validates
    // the Bearer token stored in local `settings.json`, emits a tool_call (`read`), and
    // returns the final assistant reply on turn 2.
    let turn_counter = Arc::new(AtomicUsize::new(0));
    let tc_clone = turn_counter.clone();
    let expected_key = "sk-local-test-secret-9876";

    let mock_app = Router::new()
        .route(
            "/v1/models",
            get(move |headers: HeaderMap| async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if auth != format!("Bearer {expected_key}") {
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({"error": "missing bearer"})),
                    );
                }
                (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "data": [{ "id": "qwen3.8-27b", "context_window": 262144 }]
                    })),
                )
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let tc = tc_clone.clone();
                async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if auth != format!("Bearer {expected_key}") {
                        return (
                            axum::http::StatusCode::UNAUTHORIZED,
                            [("content-type", "application/json")],
                            r#"{"error":{"message":"invalid api key"}}"#.to_string(),
                        )
                            .into_response();
                    }
                    let has_tool_result = body["messages"]
                        .as_array()
                        .is_some_and(|arr| arr.iter().any(|m| m["role"] == "tool"));
                    let step = tc.fetch_add(1, Ordering::SeqCst);
                    let sse = if !has_tool_result && step == 0 {
                        concat!(
                            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_read_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"greeting.txt\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
                            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":18,\"prompt_tokens_details\":{\"cached_tokens\":80}}}\n\n",
                            "data: [DONE]\n\n"
                        )
                        .to_string()
                    } else {
                        concat!(
                            "data: {\"choices\":[{\"delta\":{\"content\":\"Verified greeting.txt via live AgentRunner.\"},\"finish_reason\":null}]}\n\n",
                            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":200,\"completion_tokens\":12,\"prompt_tokens_details\":{\"cached_tokens\":160}}}\n\n",
                            "data: [DONE]\n\n"
                        )
                        .to_string()
                    };
                    (
                        axum::http::StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        sse,
                    )
                        .into_response()
                }
            }),
        );

    let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_addr = mock_listener.local_addr().unwrap();
    let mock_handle = tokio::spawn(async move {
        let _ = axum::serve(mock_listener, mock_app).await;
    });

    // 1. `saint-harness auth login` writes ~/.saint-harness/settings.json (chmod 0600) and verifies /v1/models live!
    let mut login_child = Command::new(bin_path())
        .args([
            "--home-dir",
            home.to_str().unwrap(),
            "auth",
            "login",
            "--provider",
            "local",
            "--base-url",
            &format!("http://{mock_addr}/v1"),
            "--api-key-stdin",
            "--set-default",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    login_child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{expected_key}\n").as_bytes())
        .unwrap();
    let login_out = login_child.wait_with_output().unwrap();
    assert!(
        login_out.status.success(),
        "auth login failed: {}",
        String::from_utf8_lossy(&login_out.stderr)
    );
    let login_json: Value = serde_json::from_slice(&login_out.stdout).unwrap();
    assert_eq!(login_json["saved"], Value::Bool(true));
    assert_eq!(login_json["verified"], Value::Bool(true));
    assert!(home.join("settings.json").exists());

    // 2. `saint-harness auth status --json` shows masked preview and `settings.json` provenance
    let status_auth = Command::new(bin_path())
        .args([
            "--home-dir",
            home.to_str().unwrap(),
            "auth",
            "status",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(status_auth.status.success());
    let auth_json: Value = serde_json::from_slice(&status_auth.stdout).unwrap();
    assert_eq!(auth_json["settings_exists"], Value::Bool(true));
    let local_route = auth_json["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["route"] == "qwen_local_sglang")
        .unwrap();
    assert_eq!(local_route["configured"], Value::Bool(true));
    assert_eq!(local_route["source"], "settings.json");
    assert_eq!(local_route["preview"], "sk-loc...9876");

    // 3. `saint-harness models --json` (zero hardcoded provider versions)
    let models_out = Command::new(bin_path())
        .args(["--home-dir", home.to_str().unwrap(), "models", "--json"])
        .output()
        .unwrap();
    assert!(models_out.status.success());
    let models_json: Value = serde_json::from_slice(&models_out.stdout).unwrap();
    assert_eq!(models_json["hardcodedProviderVersions"], 0);
    assert_eq!(models_json["profiles"].as_array().unwrap().len(), 4);

    // 4. Live `saint-harness run` executing real tool call (`read`) + live SSE turn via settings.json!
    let run_out = Command::new(bin_path())
        .current_dir(&ws)
        .env("SAINT_HARNESS_HOME", &home)
        .args([
            "--home-dir",
            home.to_str().unwrap(),
            "run",
            "Read greeting.txt and verify",
            "--auto",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        run_out.status.success(),
        "live run failed: stdout={} stderr={}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr)
    );
    let run_json: Value = serde_json::from_slice(&run_out.stdout).unwrap();
    assert_eq!(run_json["event"], "session.completed");
    assert_eq!(run_json["steps_executed"], 2);
    assert!(
        run_json["final_text"]
            .as_str()
            .unwrap()
            .contains("Verified greeting.txt")
    );

    // 5. Live `saint-harness eval` running task + verify_command -> pass_at_1 == 1.0!
    let suite_path = tmp.path().join("suite.yaml");
    #[cfg(unix)]
    let verify_cmd = "test -f greeting.txt";
    #[cfg(windows)]
    let verify_cmd = "if exist greeting.txt (exit /b 0) else (exit /b 1)";
    std::fs::write(
        &suite_path,
        format!(
            "name: live-smoke\ntasks:\n  - id: t1\n    prompt: Verify greeting\n    verify_command: \"{verify_cmd}\"\n"
        ),
    )
    .unwrap();
    let eval_out = Command::new(bin_path())
        .current_dir(&ws)
        .env("SAINT_HARNESS_HOME", &home)
        .args([
            "--home-dir",
            home.to_str().unwrap(),
            "eval",
            suite_path.to_str().unwrap(),
            "--profiles",
            "qwen-3.8-27b",
            "--edit",
            "hashline",
            "--repeat",
            "1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        eval_out.status.success(),
        "live eval failed: {}",
        String::from_utf8_lossy(&eval_out.stderr)
    );
    let eval_json: Value = serde_json::from_slice(&eval_out.stdout).unwrap();
    assert_eq!(eval_json["suite"], "live-smoke");
    assert_eq!(eval_json["rows"][0]["pass_at_1"], 1.0);
    assert!(eval_json["rows"][0]["cache_hit_ratio"].as_f64().unwrap() > 0.5);

    // 6. `saint-harness auth logout` cleanly removes key from settings.json
    let logout_out = Command::new(bin_path())
        .args([
            "--home-dir",
            home.to_str().unwrap(),
            "auth",
            "logout",
            "local",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(logout_out.status.success());
    let logout_json: Value = serde_json::from_slice(&logout_out.stdout).unwrap();
    assert_eq!(logout_json["removed"], Value::Bool(true));

    // 7. Live `saint-harness-server` -> `saint-harness server status --json`
    let state = saint_harness_server::AppState::bootstrap(
        ws.clone(),
        home.clone(),
        home.join("server-data"),
        "http://127.0.0.1:0".into(),
        None,
        false,
    )
    .await
    .unwrap();
    let (bound, handle) =
        saint_harness_server::serve_tcp("127.0.0.1:0".parse().unwrap(), state, None)
            .await
            .unwrap();

    let status_out = Command::new(bin_path())
        .args([
            "server",
            "status",
            "--url",
            &format!("http://{bound}"),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        status_out.status.success(),
        "live server status failed: {}",
        String::from_utf8_lossy(&status_out.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status_out.stdout).unwrap();
    assert_eq!(status_json["reachable"], Value::Bool(true));
    assert_eq!(status_json["ready"], Value::Bool(true));
    assert_eq!(status_json["projection_state"], "healthy");

    handle.abort();
    mock_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_doctor_and_self_update_e2e_verifies_sha256_and_replaces_binary() {
    use axum::Router;
    use axum::routing::get;
    use sha2::{Digest, Sha256};

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // 1. `saint-harness doctor --json`
    let doc_out = Command::new(bin_path())
        .args(["--home-dir", home.to_str().unwrap(), "doctor", "--json"])
        .output()
        .unwrap();
    assert!(doc_out.status.success());
    let doc_json: Value = serde_json::from_slice(&doc_out.stdout).unwrap();
    assert_eq!(doc_json["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(doc_json["settings_permissions_ok"], Value::Bool(true));

    // 2. Build a real release archive for the current platform and serve release-manifest.json
    let stage = tmp.path().join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    let (platform_key, archive_name, bin_name) =
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => (
                "linux-x86_64",
                "saint-harness_9.9.9_linux-x86_64.tar.gz",
                "saint-harness",
            ),
            ("linux", "aarch64") => (
                "linux-aarch64",
                "saint-harness_9.9.9_linux-aarch64.tar.gz",
                "saint-harness",
            ),
            ("macos", "aarch64") => (
                "darwin-aarch64",
                "saint-harness_9.9.9_darwin-arm64.tar.gz",
                "saint-harness",
            ),
            ("macos", "x86_64") => (
                "darwin-x86_64",
                "saint-harness_9.9.9_darwin-x86_64.tar.gz",
                "saint-harness",
            ),
            ("windows", "x86_64") => (
                "windows-x86_64",
                "saint-harness_9.9.9_windows-x86_64.zip",
                "saint-harness.exe",
            ),
            other => panic!("unsupported test platform {other:?}"),
        };

    let fake_new_payload = b"#!/bin/sh\necho saint-harness-9.9.9-updated\n";
    let staged_bin = stage.join(bin_name);
    std::fs::write(&staged_bin, fake_new_payload).unwrap();
    let archive_path = tmp.path().join(archive_name);

    #[cfg(unix)]
    {
        let st = Command::new("tar")
            .args([
                "-C",
                stage.to_str().unwrap(),
                "-czf",
                archive_path.to_str().unwrap(),
                bin_name,
            ])
            .status()
            .unwrap();
        assert!(st.success());
    }
    #[cfg(windows)]
    {
        let st = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
                    staged_bin.display(),
                    archive_path.display()
                ),
            ])
            .status()
            .unwrap();
        assert!(st.success());
    }

    let archive_bytes = std::fs::read(&archive_path).unwrap();
    let sha_hex: String = Sha256::digest(&archive_bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dl_url = format!("http://{addr}/download/{archive_name}");
    let good_manifest = serde_json::json!({
        "schemaVersion": 1,
        "name": "saint-harness",
        "version": "9.9.9",
        "tag": "v9.9.9",
        "platforms": {
            platform_key: {
                "filename": archive_name,
                "url": dl_url,
                "sha256": sha_hex,
                "size": archive_bytes.len()
            }
        }
    });
    let bad_manifest = serde_json::json!({
        "schemaVersion": 1,
        "name": "saint-harness",
        "version": "9.9.9",
        "tag": "v9.9.9",
        "platforms": {
            platform_key: {
                "filename": archive_name,
                "url": dl_url,
                "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "size": archive_bytes.len()
            }
        }
    });

    let bytes_clone = archive_bytes.clone();
    let app = Router::new()
        .route(
            "/manifest.json",
            get(move || {
                let m = good_manifest.clone();
                async move { axum::Json(m) }
            }),
        )
        .route(
            "/bad-manifest.json",
            get(move || {
                let m = bad_manifest.clone();
                async move { axum::Json(m) }
            }),
        )
        .route(
            &format!("/download/{archive_name}"),
            get(move || {
                let b = bytes_clone.clone();
                async move { b }
            }),
        );
    let srv = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let target_bin = tmp.path().join("installed").join(bin_name);
    std::fs::create_dir_all(target_bin.parent().unwrap()).unwrap();
    std::fs::write(&target_bin, b"old-binary").unwrap();

    // 3. `--check` detects 9.9.9 without touching target_bin
    let chk = Command::new(bin_path())
        .args([
            "update",
            "--check",
            "--manifest-url",
            &format!("http://{addr}/manifest.json"),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(chk.status.success());
    let chk_json: Value = serde_json::from_slice(&chk.stdout).unwrap();
    assert_eq!(chk_json["update_available"], Value::Bool(true));
    assert_eq!(chk_json["latest_version"], "9.9.9");
    assert_eq!(std::fs::read(&target_bin).unwrap(), b"old-binary");

    // 4. Bad SHA-256 fails closed without modifying target_bin
    let bad = Command::new(bin_path())
        .args([
            "update",
            "--manifest-url",
            &format!("http://{addr}/bad-manifest.json"),
            "--target-bin",
            target_bin.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("SHA-256 verification failed"));
    assert_eq!(std::fs::read(&target_bin).unwrap(), b"old-binary");

    // 5. Valid SHA-256 downloads, verifies, extracts, and atomically replaces target_bin
    let ok = Command::new(bin_path())
        .args([
            "update",
            "--manifest-url",
            &format!("http://{addr}/manifest.json"),
            "--target-bin",
            target_bin.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        ok.status.success(),
        "update failed: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let ok_json: Value = serde_json::from_slice(&ok.stdout).unwrap();
    assert_eq!(ok_json["updated"], Value::Bool(true));
    assert_eq!(ok_json["installed_version"], "9.9.9");
    assert_eq!(std::fs::read(&target_bin).unwrap(), fake_new_payload);

    srv.abort();
}
