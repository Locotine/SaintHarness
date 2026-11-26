use saint_harness_schema::SandboxProfile;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreated,
    RulesetCreatedAttr,
};

#[cfg(target_os = "linux")]
pub struct PreparedLinuxSandbox {
    pub ruleset: RulesetCreated,
}

/// Builds the complete Landlock ruleset and opens all `PathFd`s (`O_CLOEXEC`) in the parent process
/// BEFORE `fork()`/`spawn()`. Inside `pre_exec`, only `ruleset.restrict_self()` is invoked, keeping
/// `pre_exec` 100% async-signal-safe with zero heap allocations.
#[cfg(target_os = "linux")]
pub fn prepare_linux_sandbox(
    workspace_root: &Path,
    profile: &SandboxProfile,
) -> Result<Option<PreparedLinuxSandbox>, String> {
    if matches!(profile, SandboxProfile::Disabled) {
        return Ok(None);
    }

    let abi = ABI::V3;
    let all_fs = AccessFs::from_all(abi);
    let ro_fs = AccessFs::from_read(abi);

    let mut ruleset = Ruleset::default()
        .handle_access(all_fs)
        .map_err(|e| format!("landlock handle_access failed: {e}"))?
        .create()
        .map_err(|e| format!("landlock ruleset create failed: {e}"))?;

    // System read-only runtime directories required for spawning shells and standard tools.
    let mut ro_paths: Vec<PathBuf> = vec![
        PathBuf::from("/usr"),
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/lib"),
        PathBuf::from("/lib64"),
        PathBuf::from("/etc"),
        PathBuf::from("/proc"),
        PathBuf::from("/dev/null"),
        PathBuf::from("/dev/zero"),
        PathBuf::from("/dev/urandom"),
        workspace_root.to_path_buf(),
    ];

    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        for sub in [".cargo", ".rustup", ".nvm", ".local"] {
            let p = home_path.join(sub);
            if p.exists() {
                ro_paths.push(p);
            }
        }
    }

    for p in ro_paths {
        if p.exists()
            && let Ok(fd) = PathFd::new(&p)
        {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, ro_fs))
                .map_err(|e| format!("landlock ro rule for {} failed: {e}", p.display()))?;
        }
    }

    // Always allow writing to /dev/null
    if let Ok(fd) = PathFd::new("/dev/null") {
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, all_fs))
            .map_err(|e| format!("landlock /dev/null rule failed: {e}"))?;
    }

    if let SandboxProfile::WorkspaceWrite { extra_rw_dirs } = profile {
        let mut rw_paths = vec![workspace_root.to_path_buf()];
        rw_paths.extend(extra_rw_dirs.iter().cloned());
        for p in rw_paths {
            if p.exists()
                && let Ok(fd) = PathFd::new(&p)
            {
                ruleset = ruleset
                    .add_rule(PathBeneath::new(fd, all_fs))
                    .map_err(|e| format!("landlock rw rule for {} failed: {e}", p.display()))?;
            }
        }
    }

    Ok(Some(PreparedLinuxSandbox { ruleset }))
}

#[cfg(not(target_os = "linux"))]
pub struct PreparedLinuxSandbox;

#[cfg(not(target_os = "linux"))]
pub fn prepare_linux_sandbox(
    _workspace_root: &Path,
    _profile: &SandboxProfile,
) -> Result<Option<PreparedLinuxSandbox>, String> {
    Ok(None)
}
