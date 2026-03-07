use super::types::{Permission, Role};

/// Return the set of permissions granted to a role.
pub fn permissions_for_role(role: Role) -> &'static [Permission] {
    match role {
        Role::Admin => &[
            Permission::Inference,
            Permission::ReadStats,
            Permission::ReadWaste,
            Permission::ManageKeys,
            Permission::ManageRouting,
            Permission::ManageAlerts,
            Permission::ManageConfig,
            Permission::ExportCompliance,
            Permission::ManageUsers,
            Permission::ManageExperiments,
            Permission::ManagePrompts,
        ],
        Role::Operator => &[
            Permission::Inference,
            Permission::ReadStats,
            Permission::ReadWaste,
            Permission::ManageKeys,
            Permission::ManageRouting,
            Permission::ManageAlerts,
            Permission::ManageConfig,
            Permission::ManageExperiments,
            Permission::ManagePrompts,
        ],
        Role::Analyst => &[
            Permission::Inference,
            Permission::ReadStats,
            Permission::ReadWaste,
            Permission::ExportCompliance,
        ],
        Role::Viewer => &[Permission::ReadStats, Permission::ReadWaste],
    }
}

/// Check if a role has a specific permission.
pub fn has_permission(role: Role, permission: Permission) -> bool {
    permissions_for_role(role).contains(&permission)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_has_all_permissions() {
        assert!(has_permission(Role::Admin, Permission::ManageUsers));
        assert!(has_permission(Role::Admin, Permission::Inference));
        assert!(has_permission(Role::Admin, Permission::ExportCompliance));
    }

    #[test]
    fn viewer_limited() {
        assert!(has_permission(Role::Viewer, Permission::ReadStats));
        assert!(has_permission(Role::Viewer, Permission::ReadWaste));
        assert!(!has_permission(Role::Viewer, Permission::Inference));
        assert!(!has_permission(Role::Viewer, Permission::ManageKeys));
    }

    #[test]
    fn operator_no_manage_users() {
        assert!(has_permission(Role::Operator, Permission::ManageKeys));
        assert!(!has_permission(Role::Operator, Permission::ManageUsers));
        assert!(!has_permission(
            Role::Operator,
            Permission::ExportCompliance
        ));
    }

    #[test]
    fn analyst_can_export() {
        assert!(has_permission(Role::Analyst, Permission::ExportCompliance));
        assert!(has_permission(Role::Analyst, Permission::Inference));
        assert!(!has_permission(Role::Analyst, Permission::ManageKeys));
    }
}
