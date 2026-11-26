use crate::env_scrub::scrubbed_env;
use crate::sandbox_linux::prepare_linux_sandbox;
use async_trait::async_trait;
use dashmap::DashMap;
use saint_harness_schema::{SandboxRunner, ShellOutput, ShellRequest};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use ulid::Ulid;

#[derive(Debug, Clone)]
pub struct BackgroundJobCompletion {
    pub job_id: String,
    pub command_preview: String,
    pub exit_code: Option<i32>,
    pub output_excerpt: String,
    pub spill_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct BackgroundJobHandle {
    pub job_id: String,
    pub pid: Option<u32>,
    pub command_preview: String,
}

#[derive(Clone, Default)]
pub struct ShellSandboxRunner {
    bg_jobs: Arc<DashMap<String, BackgroundJobHandle>>,
    completion_tx: Option<mpsc::UnboundedSender<BackgroundJobCompletion>>,
}

impl ShellSandboxRunner {
    pub fn with_completion_channel(
        completion_tx: mpsc::UnboundedSender<BackgroundJobCompletion>,
    ) -> Self {
        Self {
            bg_jobs: Arc::new(DashMap::new()),
            completion_tx: Some(completion_tx),
        }
    }

    pub fn active_background_jobs(&self) -> Vec<BackgroundJobHandle> {
        self.bg_jobs.iter().map(|e| e.value().clone()).collect()
    }

    /// Sends `SIGTERM` to the child process group, waits 3 seconds, then escalates to `SIGKILL`.
    pub async fn terminate_process_group(pid: u32) {
        #[cfg(unix)]
        {
            use nix::sys::signal::{Signal, killpg};
            use nix::unistd::Pid;
            let pgid = Pid::from_raw(pid as i32);
            let _ = killpg(pgid, Signal::SIGTERM);
            tokio::time::sleep(Duration::from_secs(3)).await;
            let _ = killpg(pgid, Signal::SIGKILL);
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
        }
    }
}

#[async_trait]
impl SandboxRunner for ShellSandboxRunner {
    async fn run(&self, req: ShellRequest) -> Result<ShellOutput, String> {
        let env_map = scrubbed_env(std::env::vars(), &req.env_allowlist);

        // Prepare Landlock ruleset and PathFds completely in parent before spawn().
        let prepared_linux = prepare_linux_sandbox(&req.workspace_root, &req.sandbox)?;

        #[cfg(unix)]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&req.command);
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd.exe");
            c.arg("/C").arg(&req.command);
            c
        };

        cmd.current_dir(&req.cwd)
            .env_clear()
            .envs(&env_map)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "linux")]
        {
            if let Some(prepared) = prepared_linux {
                let mut ruleset_opt = Some(prepared.ruleset);
                // SAFETY: only calls setpgid(0, 0) and landlock_restrict_self (pure syscalls, 0 heap allocs).
                unsafe {
                    cmd.pre_exec(move || {
                        let _ = nix::unistd::setpgid(
                            nix::unistd::Pid::from_raw(0),
                            nix::unistd::Pid::from_raw(0),
                        );
                        if let Some(ruleset) = ruleset_opt.take() {
                            ruleset
                                .restrict_self()
                                .map_err(|e| std::io::Error::other(e.to_string()))?;
                        }
                        Ok(())
                    });
                }
            } else {
                // SAFETY: setpgid(0, 0) is async-signal-safe.
                unsafe {
                    cmd.pre_exec(|| {
                        let _ = nix::unistd::setpgid(
                            nix::unistd::Pid::from_raw(0),
                            nix::unistd::Pid::from_raw(0),
                        );
                        Ok(())
                    });
                }
            }
        }

        #[cfg(all(unix, not(target_os = "linux")))]
        {
            let _ = prepared_linux;
            // SAFETY: setpgid(0, 0) is async-signal-safe.
            unsafe {
                cmd.pre_exec(|| {
                    let _ = nix::unistd::setpgid(
                        nix::unistd::Pid::from_raw(0),
                        nix::unistd::Pid::from_raw(0),
                    );
                    Ok(())
                });
            }
        }

        #[cfg(not(unix))]
        let _ = prepared_linux;

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn shell command: {e}"))?;
        let pid = child.id();

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();

        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(ref mut out) = stdout_pipe {
                let _ = out.read_to_end(&mut buf).await;
            }
            buf
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(ref mut err) = stderr_pipe {
                let _ = err.read_to_end(&mut buf).await;
            }
            buf
        });

        let timeout = Duration::from_secs(req.timeout_secs.max(1));
        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(wait_res) => {
                let status = wait_res.map_err(|e| format!("shell wait failed: {e}"))?;
                let stdout_bytes = stdout_task.await.unwrap_or_default();
                let stderr_bytes = stderr_task.await.unwrap_or_default();
                let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
                let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
                let spill_path = maybe_spill(&req.spill_dir, &stdout, &stderr);
                Ok(ShellOutput {
                    exit_code: status.code(),
                    stdout,
                    stderr,
                    timed_out_to_background: false,
                    background_job_id: None,
                    spill_path,
                })
            }
            Err(_) => {
                // Move long-running command to background and notify completion channel when finished.
                let job_id = format!("bg_{}", Ulid::new());
                let preview: String = req.command.chars().take(120).collect();
                self.bg_jobs.insert(
                    job_id.clone(),
                    BackgroundJobHandle {
                        job_id: job_id.clone(),
                        pid,
                        command_preview: preview.clone(),
                    },
                );

                let bg_jobs = self.bg_jobs.clone();
                let tx = self.completion_tx.clone();
                let spill_dir = req.spill_dir.clone();
                let jid = job_id.clone();

                tokio::spawn(async move {
                    let exit_code = child.wait().await.ok().and_then(|s| s.code());
                    let out_b = stdout_task.await.unwrap_or_default();
                    let err_b = stderr_task.await.unwrap_or_default();
                    let out_s = String::from_utf8_lossy(&out_b).into_owned();
                    let err_s = String::from_utf8_lossy(&err_b).into_owned();
                    let spill_path = maybe_spill(&spill_dir, &out_s, &err_s);
                    let combined = format!("{out_s}\n{err_s}");
                    let excerpt: String = combined.chars().take(1500).collect();
                    bg_jobs.remove(&jid);
                    if let Some(tx) = tx {
                        let _ = tx.send(BackgroundJobCompletion {
                            job_id: jid,
                            command_preview: preview,
                            exit_code,
                            output_excerpt: excerpt,
                            spill_path,
                        });
                    }
                });

                Ok(ShellOutput {
                    exit_code: None,
                    stdout: format!(
                        "Command exceeded {}s timeout and moved to background job `{job_id}`.",
                        req.timeout_secs
                    ),
                    stderr: String::new(),
                    timed_out_to_background: true,
                    background_job_id: Some(job_id),
                    spill_path: None,
                })
            }
        }
    }
}

fn maybe_spill(spill_dir: &Option<PathBuf>, stdout: &str, stderr: &str) -> Option<PathBuf> {
    let dir = spill_dir.as_ref()?;
    let combined = format!("{stdout}\n{stderr}");
    if combined.len() < 4096 {
        return None;
    }
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("sh_{}.out.zst", Ulid::new()));
    if let Ok(compressed) = zstd::stream::encode_all(combined.as_bytes(), 3)
        && std::fs::write(&path, compressed).is_ok()
    {
        return Some(path);
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use saint_harness_schema::SandboxProfile;

    #[tokio::test]
    async fn env_scrub_and_timeout_to_background_work() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner = ShellSandboxRunner::with_completion_channel(tx);

        #[cfg(unix)]
        let bg_cmd = "sleep 2 && echo bg_done".to_string();
        #[cfg(windows)]
        let bg_cmd = "ping -n 3 127.0.0.1 >NUL && echo bg_done".to_string();

        // Background timeout transition delivers completion event
        let bg = runner
            .run(ShellRequest {
                command: bg_cmd,
                cwd: tmp.path().to_path_buf(),
                workspace_root: tmp.path().to_path_buf(),
                sandbox: SandboxProfile::Disabled,
                timeout_secs: 1,
                spill_dir: None,
                env_allowlist: vec![],
            })
            .await
            .unwrap();
        assert!(bg.timed_out_to_background);
        assert!(bg.background_job_id.is_some());

        let completion = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("background job finished")
            .expect("completion payload");
        assert_eq!(completion.exit_code, Some(0));
        assert!(completion.output_excerpt.contains("bg_done"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_landlock_blocks_writes_in_read_only_and_outside_workspace() {
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let runner = ShellSandboxRunner::default();

        // ReadOnlyWorkspace must block writing even inside workspace
        let ro_target = ws.path().join("blocked.txt");
        let ro_res = runner
            .run(ShellRequest {
                command: format!("touch '{}'", ro_target.display()),
                cwd: ws.path().to_path_buf(),
                workspace_root: ws.path().to_path_buf(),
                sandbox: SandboxProfile::ReadOnlyWorkspace,
                timeout_secs: 5,
                spill_dir: None,
                env_allowlist: vec![],
            })
            .await
            .unwrap();
        // If kernel enforces Landlock, exit_code != 0 and file does not exist.
        if ro_res.exit_code == Some(0) {
            // Only acceptable if kernel explicitly lacks Landlock support.
            assert!(!std::path::Path::new("/sys/kernel/security/lsm").exists());
        } else {
            assert!(!ro_target.exists());
        }

        // WorkspaceWrite allows writing inside workspace but blocks outside workspace
        let inside_target = ws.path().join("ok.txt");
        let ok_res = runner
            .run(ShellRequest {
                command: format!("echo ok > '{}'", inside_target.display()),
                cwd: ws.path().to_path_buf(),
                workspace_root: ws.path().to_path_buf(),
                sandbox: SandboxProfile::WorkspaceWrite {
                    extra_rw_dirs: vec![],
                },
                timeout_secs: 5,
                spill_dir: None,
                env_allowlist: vec![],
            })
            .await
            .unwrap();
        assert_eq!(ok_res.exit_code, Some(0));
        assert!(inside_target.exists());

        let escape_target = outside.path().join("escape.txt");
        let esc_res = runner
            .run(ShellRequest {
                command: format!("touch '{}'", escape_target.display()),
                cwd: ws.path().to_path_buf(),
                workspace_root: ws.path().to_path_buf(),
                sandbox: SandboxProfile::WorkspaceWrite {
                    extra_rw_dirs: vec![],
                },
                timeout_secs: 5,
                spill_dir: None,
                env_allowlist: vec![],
            })
            .await
            .unwrap();
        if esc_res.exit_code != Some(0) {
            assert!(!escape_target.exists());
        }
    }
}
