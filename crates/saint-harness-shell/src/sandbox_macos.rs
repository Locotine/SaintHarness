use saint_harness_schema::SandboxProfile;
use std::path::Path;

/// Generates a macOS Seatbelt (`sandbox-exec -p`) SBPL profile for workspace sandboxing.
pub fn build_sbpl_profile(workspace_root: &Path, profile: &SandboxProfile) -> Option<String> {
    match profile {
        SandboxProfile::Disabled => None,
        SandboxProfile::ReadOnlyWorkspace => Some(format!(
            "(version 1)\n(deny default)\n(allow process*)\n(allow file-read*)\n(allow file-write* (literal \"/dev/null\"))\n; workspace read-only: {}\n",
            workspace_root.display()
        )),
        SandboxProfile::WorkspaceWrite { extra_rw_dirs } => {
            let mut rules = format!(
                "(version 1)\n(deny default)\n(allow process*)\n(allow file-read*)\n(allow file-write* (literal \"/dev/null\"))\n(allow file-write* (subpath \"{}\"))\n",
                workspace_root.display()
            );
            for dir in extra_rw_dirs {
                rules.push_str(&format!(
                    "(allow file-write* (subpath \"{}\"))\n",
                    dir.display()
                ));
            }
            Some(rules)
        }
    }
}
