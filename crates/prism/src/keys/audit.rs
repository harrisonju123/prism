#[cfg(feature = "postgres")]
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
pub enum AuditEventType {
    KeyCreated,
    KeyUpdated,
    KeyRevoked,
    KeyRotated,
    AuthFailure,
    RateLimitHit,
    BudgetExceeded,
}

impl AuditEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KeyCreated => "key_created",
            Self::KeyUpdated => "key_updated",
            Self::KeyRevoked => "key_revoked",
            Self::KeyRotated => "key_rotated",
            Self::AuthFailure => "auth_failure",
            Self::RateLimitHit => "rate_limit_hit",
            Self::BudgetExceeded => "budget_exceeded",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_strings_match_expected() {
        assert_eq!(AuditEventType::KeyCreated.as_str(), "key_created");
        assert_eq!(AuditEventType::KeyUpdated.as_str(), "key_updated");
        assert_eq!(AuditEventType::KeyRevoked.as_str(), "key_revoked");
        assert_eq!(AuditEventType::KeyRotated.as_str(), "key_rotated");
        assert_eq!(AuditEventType::AuthFailure.as_str(), "auth_failure");
        assert_eq!(AuditEventType::RateLimitHit.as_str(), "rate_limit_hit");
        assert_eq!(AuditEventType::BudgetExceeded.as_str(), "budget_exceeded");
    }
}

#[cfg(feature = "postgres")]
pub struct AuditService {
    pool: PgPool,
}

#[cfg(feature = "postgres")]
impl AuditService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fire-and-forget audit log write — never blocks the request path.
    pub fn log(
        &self,
        event_type: AuditEventType,
        key_id: Option<Uuid>,
        key_prefix: Option<String>,
        actor: Option<String>,
        details: serde_json::Value,
        ip_addr: Option<String>,
    ) {
        let pool = self.pool.clone();
        let event_type_str = event_type.as_str().to_string();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query(
                r#"INSERT INTO audit_events (event_type, key_id, key_prefix, actor, details, ip_addr)
                   VALUES ($1, $2, $3, $4, $5, $6)"#,
            )
            .bind(&event_type_str)
            .bind(key_id)
            .bind(&key_prefix)
            .bind(&actor)
            .bind(&details)
            .bind(&ip_addr)
            .execute(&pool)
            .await
            {
                tracing::warn!(error = %e, "audit log write failed");
            }
        });
    }
}

/// Stub for non-Postgres builds.
#[cfg(not(feature = "postgres"))]
pub struct AuditService;

#[cfg(not(feature = "postgres"))]
impl AuditService {
    pub fn log(
        &self,
        _event_type: AuditEventType,
        _key_id: Option<Uuid>,
        _key_prefix: Option<String>,
        _actor: Option<String>,
        _details: serde_json::Value,
        _ip_addr: Option<String>,
    ) {
    }
}
