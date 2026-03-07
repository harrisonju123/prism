use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};

use crate::config::JwtConfig;

/// Standard JWT claims mapped to PrisM auth context fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrismClaims {
    /// Subject (key or user ID)
    pub sub: String,
    /// Team/tenant ID
    #[serde(default)]
    pub team_id: Option<String>,
    /// Allowed models (comma-separated or array)
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    /// Role (admin/operator/analyst/viewer)
    #[serde(default)]
    pub role: Option<String>,
    /// Expiration time (Unix timestamp)
    pub exp: u64,
    /// Issued at (Unix timestamp)
    #[serde(default)]
    pub iat: Option<u64>,
    /// Issuer
    #[serde(default)]
    pub iss: Option<String>,
}

/// Validate a JWT token and extract claims.
pub fn validate_jwt(token: &str, config: &JwtConfig) -> Result<PrismClaims, String> {
    let algorithm = match config.algorithm.as_str() {
        "HS256" => Algorithm::HS256,
        "HS384" => Algorithm::HS384,
        "HS512" => Algorithm::HS512,
        "RS256" => Algorithm::RS256,
        "RS384" => Algorithm::RS384,
        "RS512" => Algorithm::RS512,
        other => return Err(format!("unsupported JWT algorithm: {other}")),
    };

    let decoding_key = match algorithm {
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
            let secret = config
                .secret
                .as_ref()
                .ok_or("JWT secret not configured for HMAC algorithm")?;
            DecodingKey::from_secret(secret.as_bytes())
        }
        Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 => {
            let pem = config
                .public_key_pem
                .as_ref()
                .ok_or("JWT public_key_pem not configured for RSA algorithm")?;
            DecodingKey::from_rsa_pem(pem.as_bytes())
                .map_err(|e| format!("invalid RSA public key: {e}"))?
        }
        _ => return Err(format!("unsupported algorithm: {:?}", algorithm)),
    };

    let mut validation = Validation::new(algorithm);
    if let Some(ref issuer) = config.issuer {
        validation.set_issuer(&[issuer]);
    }

    let token_data = decode::<PrismClaims>(token, &decoding_key, &validation)
        .map_err(|e| format!("JWT validation failed: {e}"))?;

    Ok(token_data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    fn make_config(secret: &str) -> JwtConfig {
        JwtConfig {
            enabled: true,
            algorithm: "HS256".into(),
            secret: Some(secret.into()),
            public_key_pem: None,
            issuer: None,
        }
    }

    #[test]
    fn valid_hs256_token() {
        let config = make_config("test-secret-key-at-least-32-bytes!");
        let claims = PrismClaims {
            sub: "user-123".into(),
            team_id: Some("team-1".into()),
            allowed_models: Some(vec!["gpt-4o".into()]),
            role: Some("operator".into()),
            exp: chrono::Utc::now().timestamp() as u64 + 3600,
            iat: Some(chrono::Utc::now().timestamp() as u64),
            iss: None,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"test-secret-key-at-least-32-bytes!"),
        )
        .unwrap();

        let result = validate_jwt(&token, &config);
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        let decoded = result.unwrap();
        assert_eq!(decoded.sub, "user-123");
        assert_eq!(decoded.team_id, Some("team-1".into()));
    }

    #[test]
    fn expired_token_rejected() {
        let config = make_config("test-secret-key-at-least-32-bytes!");
        let claims = PrismClaims {
            sub: "user".into(),
            team_id: None,
            allowed_models: None,
            role: None,
            exp: 1000, // way in the past
            iat: None,
            iss: None,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"test-secret-key-at-least-32-bytes!"),
        )
        .unwrap();

        let result = validate_jwt(&token, &config);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_secret_rejected() {
        let config = make_config("correct-secret-key-at-least-32bytes!");
        let claims = PrismClaims {
            sub: "user".into(),
            team_id: None,
            allowed_models: None,
            role: None,
            exp: chrono::Utc::now().timestamp() as u64 + 3600,
            iat: None,
            iss: None,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"wrong-secret-key-at-least-32bytesxx"),
        )
        .unwrap();

        let result = validate_jwt(&token, &config);
        assert!(result.is_err());
    }
}
