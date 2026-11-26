use saint_harness_schema::SandboxProfile;
use std::path::Path;

pub fn windows_sandbox_summary(workspace_root: &Path, profile: &SandboxProfile) -> String {
    match profile {
        SandboxProfile::Disabled => "Windows sandbox disabled".to_string(),
        SandboxProfile::ReadOnlyWorkspace => format!(
            "Windows JobObject + env scrub (read-only policy: {})",
            workspace_root.display()
        ),
        SandboxProfile::WorkspaceWrite { extra_rw_dirs } => format!(
            "Windows JobObject + env scrub (workspace: {}, extra rw: {})",
            workspace_root.display(),
            extra_rw_dirs.len()
        ),
    }
}
