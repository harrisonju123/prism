use serde::{Deserialize, Serialize};

/// Enterprise user roles with hierarchical permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Operator,
    Analyst,
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::Analyst => "analyst",
            Role::Viewer => "viewer",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Role::Admin),
            "operator" => Some(Role::Operator),
            "analyst" => Some(Role::Analyst),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
}

/// Granular permission types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Can make inference requests
    Inference,
    /// Can read analytics and stats
    ReadStats,
    /// Can read waste reports
    ReadWaste,
    /// Can manage API keys (create, revoke, update)
    ManageKeys,
    /// Can manage routing policies
    ManageRouting,
    /// Can manage alert rules
    ManageAlerts,
    /// Can reload configuration
    ManageConfig,
    /// Can export compliance reports
    ExportCompliance,
    /// Can manage users and roles
    ManageUsers,
    /// Can manage experiments
    ManageExperiments,
    /// Can manage prompt templates
    ManagePrompts,
}

/// A tenant user record from Postgres.
#[derive(Debug, Clone, Serialize)]
pub struct TenantUser {
    pub id: uuid::Uuid,
    pub team_id: String,
    pub email: String,
    pub role: Role,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_roundtrip() {
        for role in [Role::Admin, Role::Operator, Role::Analyst, Role::Viewer] {
            let s = role.as_str();
            assert_eq!(Role::from_str(s), Some(role));
        }
    }

    #[test]
    fn role_unknown() {
        assert_eq!(Role::from_str("superadmin"), None);
    }
}
