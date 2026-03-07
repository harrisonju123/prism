use std::time::Duration;

use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::config::CorsConfig;

/// CORS middleware — configurable origins, methods, and max-age.
pub fn cors_layer(config: &CorsConfig) -> CorsLayer {
    let origin = if config.allowed_origins.len() == 1 && config.allowed_origins[0] == "*" {
        AllowOrigin::any()
    } else {
        let origins: Vec<HeaderValue> = config
            .allowed_origins
            .iter()
            .filter_map(|o| o.parse::<HeaderValue>().ok())
            .collect();
        AllowOrigin::list(origins)
    };

    let methods: Vec<Method> = config
        .allowed_methods
        .iter()
        .filter_map(|m| m.parse::<Method>().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods(methods)
        .allow_headers(tower_http::cors::Any)
        .max_age(Duration::from_secs(config.max_age_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_default() {
        let config = CorsConfig::default();
        assert_eq!(config.allowed_origins, vec!["*"]);
        let _layer = cors_layer(&config);
    }

    #[test]
    fn specific_origins() {
        let config = CorsConfig {
            allowed_origins: vec![
                "https://example.com".into(),
                "https://app.example.com".into(),
            ],
            allowed_methods: vec!["GET".into(), "POST".into()],
            max_age_secs: 600,
        };
        let _layer = cors_layer(&config);
    }

    #[test]
    fn config_defaults() {
        let config = CorsConfig::default();
        assert_eq!(config.max_age_secs, 3600);
        assert_eq!(config.allowed_methods, vec!["GET", "POST", "OPTIONS"]);
    }
}
