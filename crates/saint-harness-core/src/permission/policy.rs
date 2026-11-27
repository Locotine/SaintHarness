use globset::Glob;
use saint_harness_schema::{
    InteractionMode, PermissionEffect, PermissionPolicy, PermissionRule, RuntimeMode,
    SandboxProfile,
};
use std::path::{Path, PathBuf};

/// Evaluates permission lattice (`Rules`, `Override`, `Restrict`, `Fallback`):
/// - Last-match-wins within each tier
/// - Combination lattice obeys `Deny > Ask > Allow` (`PermissionEffect::max`)
/// - Short-circuit Deny
/// - Supports 3 SHA-aligned `RuntimeMode`s (`approval-required`, `auto`, `full-access`) + `PlanMode` + `additional_directories`.
pub fn evaluate_permission(
    policy: &PermissionPolicy,
    workspace_root: &Path,
    action: &str,
    resource: &str,
    server_name: Option<&str>,
    saved_rules: &[PermissionRule],
) -> PermissionEffect {
    // 1. Restrict layer can only tighten (short-circuit Deny).
    if let Some(eff) = last_match_effect(&policy.restrict_rules, action, resource)
        && eff == PermissionEffect::Deny
    {
        return PermissionEffect::Deny;
    }

    // 2. Plan mode forbids mutating actions regardless of runtime mode.
    if policy.interaction_mode == InteractionMode::Plan && is_mutating_action(action) {
        return PermissionEffect::Deny;
    }

    // 3. Explicit full-access (Bypass permissions) skips prompts unless explicitly restricted above.
    if policy.runtime_mode == RuntimeMode::FullAccess {
        return PermissionEffect::Allow;
    }

    // 4. Trusted auto-approved MCP servers (e.g. "saint").
    if let Some(srv) = server_name
        && policy.auto_approve_servers.iter().any(|s| s == srv)
    {
        return PermissionEffect::Allow;
    }

    // 5. Base rules + override rules + saved approvals
    let mut base = last_match_effect(&policy.rules, action, resource);
    if let Some(ov) = last_match_effect(&policy.override_rules, action, resource) {
        base = Some(ov);
    }
    if let Some(saved) = last_match_effect(saved_rules, action, resource) {
        base = match base {
            Some(PermissionEffect::Deny) => Some(PermissionEffect::Deny),
            _ => Some(saved),
        };
    }

    let default_eff =
        base.unwrap_or_else(|| default_effect_for_mode(policy, workspace_root, action, resource));

    // Combine with restrict layer (`Deny > Ask > Allow`)
    if let Some(restrict) = last_match_effect(&policy.restrict_rules, action, resource) {
        default_eff.max(restrict)
    } else {
        default_eff
    }
}

pub fn sandbox_for_modes(
    runtime_mode: RuntimeMode,
    interaction_mode: InteractionMode,
    additional_directories: &[PathBuf],
) -> SandboxProfile {
    if interaction_mode == InteractionMode::Plan {
        return SandboxProfile::ReadOnlyWorkspace;
    }
    match runtime_mode {
        RuntimeMode::FullAccess => SandboxProfile::Disabled,
        RuntimeMode::ApprovalRequired | RuntimeMode::Auto => SandboxProfile::WorkspaceWrite {
            extra_rw_dirs: additional_directories.to_vec(),
        },
    }
}

pub fn is_action_denied(policy: &PermissionPolicy, action: &str) -> bool {
    if policy.interaction_mode == InteractionMode::Plan && is_mutating_action(action) {
        return true;
    }
    matches!(
        last_match_effect(&policy.restrict_rules, action, "*"),
        Some(PermissionEffect::Deny)
    )
}

fn is_mutating_action(action: &str) -> bool {
    matches!(
        action,
        "edit" | "write" | "ast_edit" | "replace_symbol" | "shell"
    )
}

fn default_effect_for_mode(
    policy: &PermissionPolicy,
    workspace_root: &Path,
    action: &str,
    resource: &str,
) -> PermissionEffect {
    let in_scope =
        is_resource_within_scope(workspace_root, &policy.additional_directories, resource);
    match action {
        "read" | "grep" | "glob" | "repo_map" | "symbol_read" | "find_callers" => {
            if is_sensitive_file(resource) {
                PermissionEffect::Ask
            } else if in_scope {
                PermissionEffect::Allow
            } else {
                PermissionEffect::Ask
            }
        }
        "edit" | "write" | "ast_edit" | "replace_symbol" => match policy.runtime_mode {
            RuntimeMode::Auto if in_scope && !is_sensitive_file(resource) => {
                PermissionEffect::Allow
            }
            RuntimeMode::FullAccess => PermissionEffect::Allow,
            _ => PermissionEffect::Ask,
        },
        "shell" => match policy.runtime_mode {
            RuntimeMode::Auto if is_safe_workspace_command(resource) => PermissionEffect::Allow,
            RuntimeMode::FullAccess => PermissionEffect::Allow,
            _ => PermissionEffect::Ask,
        },
        _ => policy.fallback.unwrap_or(PermissionEffect::Ask),
    }
}

fn is_resource_within_scope(
    workspace_root: &Path,
    additional_dirs: &[PathBuf],
    resource: &str,
) -> bool {
    let p = Path::new(resource);
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    if !p.is_absolute() && !p.has_root() {
        return true;
    }
    if p.starts_with(workspace_root) {
        return true;
    }
    additional_dirs.iter().any(|dir| p.starts_with(dir))
}

fn is_sensitive_file(resource: &str) -> bool {
    let lower = resource.replace('\\', "/").to_ascii_lowercase();
    lower.ends_with(".env")
        || lower.contains("/.env.")
        || lower.contains(".ssh/")
        || lower.contains("id_rsa")
        || lower.contains("credentials")
}

fn is_safe_workspace_command(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    if trimmed.contains("sudo ")
        || trimmed.contains("rm -rf /")
        || trimmed.contains("curl ")
        || trimmed.contains("wget ")
    {
        return false;
    }
    let safe_prefixes = [
        "cargo ",
        "rustc ",
        "pytest",
        "npm test",
        "pnpm test",
        "go test",
        "ls",
        "git status",
        "git diff",
        "git log",
        "echo ",
    ];
    safe_prefixes.iter().any(|p| trimmed.starts_with(p))
}

fn last_match_effect(
    rules: &[PermissionRule],
    action: &str,
    resource: &str,
) -> Option<PermissionEffect> {
    let ignore_case = cfg!(any(target_os = "macos", target_os = "windows"));
    let mut matched = None;
    for rule in rules {
        if (rule.action == "*" || rule.action == action)
            && pattern_matches(&rule.resource_pattern, resource, ignore_case)
        {
            matched = Some(rule.effect);
        }
    }
    matched
}

pub fn pattern_matches(pattern: &str, resource: &str, ignore_case: bool) -> bool {
    if pattern == "*" {
        return true;
    }
    let (pat, res) = if ignore_case {
        (pattern.to_ascii_lowercase(), resource.to_ascii_lowercase())
    } else {
        (pattern.to_string(), resource.to_string())
    };
    if let Ok(glob) = Glob::new(&pat) {
        return glob.compile_matcher().is_match(&res);
    }
    pat == res
}
