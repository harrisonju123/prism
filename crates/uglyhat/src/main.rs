use std::sync::Arc;

use uglyhat::api::AppState;
use uglyhat::server::build_router;
use uglyhat::store::sqlite::SqliteStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uglyhat=info".into()),
        )
        .init();

    let db_path = std::env::var("UGLYHAT_DB_PATH").unwrap_or_else(|_| "uglyhat.db".to_string());
    let addr = std::env::var("UGLYHAT_ADDR").unwrap_or_else(|_| "0.0.0.0:3001".to_string());

    let store = SqliteStore::open(&db_path).await?;
    let state = Arc::new(AppState {
        store: Arc::new(store),
    });
    let app = build_router(state);

    tracing::info!(addr = %addr, "uglyhat listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("shutdown signal received");
}
