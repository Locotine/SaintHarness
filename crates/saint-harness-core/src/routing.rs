use saint_harness_schema::RoutesConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRole {
    Primary,
    Deep,
    Compact,
    Vision,
    Smol,
}

pub fn profile_for_role(routes: &RoutesConfig, role: ModelRole) -> &str {
    match role {
        ModelRole::Primary => &routes.roles.primary,
        ModelRole::Deep => &routes.roles.deep,
        ModelRole::Compact => &routes.roles.compact,
        ModelRole::Vision => &routes.roles.vision,
        ModelRole::Smol => &routes.roles.smol,
    }
}
