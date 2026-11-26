use saint_harness_schema::{load_routes_with_overrides, resolve_env_value, user_settings_path};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const DEFAULT_RELEASE_REPO: &str = "Locotine/SaintHarness";

#[derive(Debug, Deserialize)]
struct ReleaseManifest {
    version: String,
    tag: String,
    platforms: BTreeMap<String, PlatformAsset>,
}

#[derive(Debug, Deserialize)]
struct PlatformAsset {
    filename: String,
    url: String,
    sha256: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub check_only: bool,
    pub force: bool,
    pub version: Option<String>,
    pub repo: String,
    pub manifest_url: Option<String>,
    pub target_bin: Option<PathBuf>,
    pub json_out: bool,
}

pub fn current_platform_key() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-aarch64"),
        ("macos", "aarch64") => Ok("darwin-aarch64"),
        ("macos", "x86_64") => Ok("darwin-x86_64"),
        ("windows", "x86_64") => Ok("windows-x86_64"),
        (os, arch) => Err(format!("unsupported platform for self-update: {os}/{arch}")),
    }
}

pub fn parse_semver_tuple(raw: &str) -> Option<(u64, u64, u64)> {
    let clean = raw.trim().trim_start_matches('v');
    let mut parts = clean.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub async fn run_update_cmd(opts: UpdateOptions) -> Result<(), String> {
    let current_ver = env!("CARGO_PKG_VERSION");
    let platform_key = current_platform_key()?;

    let manifest_url = if let Some(ref custom) = opts.manifest_url {
        custom.clone()
    } else if let Some(ref ver) = opts.version {
        let tag = if ver.starts_with('v') {
            ver.clone()
        } else {
            format!("v{ver}")
        };
        format!(
            "https://github.com/{}/releases/download/{tag}/release-manifest.json",
            opts.repo
        )
    } else {
        format!(
            "https://github.com/{}/releases/latest/download/release-manifest.json",
            opts.repo
        )
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(format!("saint-harness/{current_ver}"))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let resp = client
        .get(&manifest_url)
        .send()
        .await
        .map_err(|e| format!("failed to fetch release manifest from {manifest_url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "release manifest request returned HTTP {} ({manifest_url})",
            resp.status().as_u16()
        ));
    }
    let manifest: ReleaseManifest = resp
        .json()
        .await
        .map_err(|e| format!("invalid release-manifest.json: {e}"))?;

    let asset = manifest.platforms.get(platform_key).ok_or_else(|| {
        format!(
            "release {} does not contain asset for {platform_key}",
            manifest.tag
        )
    })?;

    let needs_update = match (
        parse_semver_tuple(current_ver),
        parse_semver_tuple(&manifest.version),
    ) {
        (Some(cur), Some(rem)) => rem > cur,
        _ => manifest.version != current_ver,
    };

    if !needs_update && !opts.force {
        if opts.json_out {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "updated": false,
                    "up_to_date": true,
                    "current_version": current_ver,
                    "latest_version": manifest.version,
                    "platform": platform_key,
                }))
                .unwrap_or_else(|_| "{}".into())
            );
        } else {
            println!(
                "saint-harness {current_ver} is already up to date (latest: {}).",
                manifest.version
            );
        }
        return Ok(());
    }

    if opts.check_only {
        if opts.json_out {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "updated": false,
                    "update_available": true,
                    "current_version": current_ver,
                    "latest_version": manifest.version,
                    "tag": manifest.tag,
                    "platform": platform_key,
                    "url": asset.url,
                    "sha256": asset.sha256,
                    "size": asset.size,
                }))
                .unwrap_or_else(|_| "{}".into())
            );
        } else {
            println!(
                "Update available: {current_ver} -> {} ({})",
                manifest.version, asset.filename
            );
            println!("Run `saint-harness update` to install.");
        }
        return Ok(());
    }

    let dl = client
        .get(&asset.url)
        .send()
        .await
        .map_err(|e| format!("failed to download {}: {e}", asset.url))?;
    if !dl.status().is_success() {
        return Err(format!(
            "download failed with HTTP {} for {}",
            dl.status().as_u16(),
            asset.url
        ));
    }
    let archive_bytes = dl
        .bytes()
        .await
        .map_err(|e| format!("failed reading archive body: {e}"))?;

    let actual_sha = sha256_hex(&archive_bytes);
    if !actual_sha.eq_ignore_ascii_case(asset.sha256.trim()) {
        return Err(format!(
            "SHA-256 verification failed for {} (expected {}, got {actual_sha})",
            asset.filename, asset.sha256
        ));
    }

    let target_bin = match opts.target_bin {
        Some(p) => p,
        None => std::env::current_exe()
            .map_err(|e| format!("failed to resolve current executable path: {e}"))?,
    };
    let parent_dir = target_bin.parent().ok_or_else(|| {
        format!(
            "target path {} has no parent directory",
            target_bin.display()
        )
    })?;
    fs::create_dir_all(parent_dir).map_err(|e| e.to_string())?;

    let stage_dir = parent_dir.join(format!(".saint-harness-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage_dir);
    fs::create_dir_all(&stage_dir).map_err(|e| e.to_string())?;

    let archive_path = stage_dir.join(&asset.filename);
    fs::write(&archive_path, &archive_bytes).map_err(|e| e.to_string())?;

    let extract_res = extract_binary(&archive_path, &stage_dir);
    let extracted_bin = match extract_res {
        Ok(p) => p,
        Err(e) => {
            let _ = fs::remove_dir_all(&stage_dir);
            return Err(e);
        }
    };

    let install_res = atomic_replace_binary(&extracted_bin, &target_bin);
    let _ = fs::remove_dir_all(&stage_dir);
    install_res?;

    if opts.json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "updated": true,
                "previous_version": current_ver,
                "installed_version": manifest.version,
                "tag": manifest.tag,
                "platform": platform_key,
                "sha256": actual_sha,
                "binary_path": target_bin.display().to_string(),
            }))
            .unwrap_or_else(|_| "{}".into())
        );
    } else {
        println!(
            "Updated saint-harness: {current_ver} -> {} ({})",
            manifest.version,
            target_bin.display()
        );
    }
    Ok(())
}

fn extract_binary(archive_path: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let fname = archive_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let bin_name = if cfg!(windows) {
        "saint-harness.exe"
    } else {
        "saint-harness"
    };

    if fname.ends_with(".tar.gz") || fname.ends_with(".tgz") {
        let out = Command::new("tar")
            .arg("-xzf")
            .arg(archive_path)
            .arg("-C")
            .arg(dest_dir)
            .output()
            .map_err(|e| format!("failed to run tar: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "tar extraction failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    } else if fname.ends_with(".zip") {
        #[cfg(windows)]
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                    archive_path.display(),
                    dest_dir.display()
                ),
            ])
            .output()
            .map_err(|e| format!("failed to run PowerShell Expand-Archive: {e}"))?;
        #[cfg(not(windows))]
        let out = Command::new("unzip")
            .arg("-o")
            .arg(archive_path)
            .arg("-d")
            .arg(dest_dir)
            .output()
            .map_err(|e| format!("failed to run unzip: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "zip extraction failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    } else {
        return Err(format!("unsupported release archive format: {fname}"));
    }

    find_file_recursive(dest_dir, bin_name)
        .ok_or_else(|| format!("extracted archive did not contain `{bin_name}`"))
}

fn find_file_recursive(dir: &Path, target_name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_file_recursive(&p, target_name) {
                return Some(found);
            }
        } else if p.file_name().and_then(|s| s.to_str()) == Some(target_name) {
            return Some(p);
        }
    }
    None
}

fn atomic_replace_binary(new_bin: &Path, target_bin: &Path) -> Result<(), String> {
    let staged = target_bin.with_extension("new");
    fs::copy(new_bin, &staged)
        .map_err(|e| format!("failed staging new binary {}: {e}", staged.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("failed chmod 0755 on {}: {e}", staged.display()))?;
        fs::rename(&staged, target_bin)
            .map_err(|e| format!("failed replacing {}: {e}", target_bin.display()))?;
    }

    #[cfg(windows)]
    {
        let backup = target_bin.with_extension("exe.old");
        let _ = fs::remove_file(&backup);
        if target_bin.exists() {
            fs::rename(target_bin, &backup)
                .map_err(|e| format!("failed rotating running exe to backup: {e}"))?;
        }
        if let Err(e) = fs::rename(&staged, target_bin) {
            let _ = fs::rename(&backup, target_bin);
            return Err(format!("failed moving updated binary into place: {e}"));
        }
    }

    Ok(())
}

pub async fn run_doctor_cmd(home_override: Option<&Path>, json_out: bool) -> Result<(), String> {
    let version = env!("CARGO_PKG_VERSION");
    let exe_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let platform = current_platform_key().unwrap_or("unknown");

    let on_path = std::env::var_os("PATH")
        .map(|paths| {
            let bin_name = if cfg!(windows) {
                "saint-harness.exe"
            } else {
                "saint-harness"
            };
            std::env::split_paths(&paths).any(|dir| dir.join(bin_name).is_file())
        })
        .unwrap_or(false);

    let settings_path = user_settings_path(home_override);
    let settings_exists = settings_path.exists();
    let settings_mode_ok = check_settings_mode(&settings_path);

    let cwd = std::env::current_dir().ok();
    let routes = load_routes_with_overrides(cwd.as_deref(), home_override).ok();
    let mut configured_routes = Vec::new();
    if let Some(ref rc) = routes {
        for (rk, r) in &rc.routes {
            if resolve_env_value(&r.api_key_env, cwd.as_deref(), home_override).is_some() {
                configured_routes.push(rk.clone());
            }
        }
    }

    let sandbox_status = detect_os_sandbox();

    let daemon_ready = match reqwest::Client::builder()
        .timeout(Duration::from_millis(800))
        .build()
    {
        Ok(client) => matches!(
            client.get("http://127.0.0.1:3774/health").send().await,
            Ok(resp) if resp.status().is_success()
        ),
        Err(_) => false,
    };

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "version": version,
                "platform": platform,
                "executable": exe_path,
                "on_path": on_path,
                "settings_path": settings_path.display().to_string(),
                "settings_exists": settings_exists,
                "settings_permissions_ok": settings_mode_ok,
                "configured_routes": configured_routes,
                "os_sandbox": sandbox_status,
                "daemon_reachable": daemon_ready,
            }))
            .unwrap_or_else(|_| "{}".into())
        );
    } else {
        println!("SaintHarness doctor ({version} · {platform})");
        println!("  ✓ binary:      {exe_path}");
        println!(
            "  {} PATH:        {}",
            if on_path { "✓" } else { "○" },
            if on_path {
                "saint-harness found on PATH"
            } else {
                "not on PATH (run `make install` or `install.sh`)"
            }
        );
        println!(
            "  {} settings:    {} ({})",
            if settings_exists && settings_mode_ok {
                "✓"
            } else {
                "○"
            },
            settings_path.display(),
            if !settings_exists {
                "not created yet; run `saint-harness auth login`"
            } else if settings_mode_ok {
                "present · 0600"
            } else {
                "present · tighten permissions with chmod 600"
            }
        );
        println!(
            "  {} credentials: {}",
            if configured_routes.is_empty() {
                "○"
            } else {
                "✓"
            },
            if configured_routes.is_empty() {
                "none configured (run `saint-harness auth login`)".to_string()
            } else {
                configured_routes.join(", ")
            }
        );
        println!("  ✓ sandbox:     {sandbox_status}");
        println!(
            "  {} daemon:      {}",
            if daemon_ready { "✓" } else { "○" },
            if daemon_ready {
                "ready at http://127.0.0.1:3774"
            } else {
                "not running (optional; start with `saint-harness server`)"
            }
        );
    }
    Ok(())
}

fn check_settings_mode(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| (m.permissions().mode() & 0o077) == 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn detect_os_sandbox() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        if let Ok(lsm) = fs::read_to_string("/sys/kernel/security/lsm")
            && lsm.contains("landlock")
        {
            return "Linux Landlock LSM active";
        }
        "Linux (Landlock LSM unavailable in kernel; fallback env-scrub)"
    }
    #[cfg(target_os = "macos")]
    {
        if Path::new("/usr/bin/sandbox-exec").exists() {
            "macOS Seatbelt (sandbox-exec available)"
        } else {
            "macOS (env-scrub)"
        }
    }
    #[cfg(windows)]
    {
        "Windows JobObject + env-scrub"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        "POSIX env-scrub"
    }
}
