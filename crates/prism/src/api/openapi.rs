use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "PrisM LLM Gateway",
        version = "0.1.0",
        description = "High-performance LLM gateway with virtual key management, model routing, and observability"
    ),
    paths(
        crate::api::health::health,
        crate::api::health::liveness,
        crate::api::health::readiness,
    ),
    tags(
        (name = "health", description = "Health check endpoints"),
        (name = "keys", description = "Virtual key management"),
        (name = "models", description = "Available models"),
    )
)]
pub struct ApiDoc;
