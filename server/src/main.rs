#![forbid(unsafe_code)]
//! Run the Ask Aunova MCP server on loopback.
//!
//! Port: `ASK_AUNOVA_PORT` (default 8731). Tokens: `ASK_AUNOVA_BUYER_TOKEN` and
//! `ASK_AUNOVA_DEVELOPER_TOKEN` (defaults are the two demo identities). Signed
//! data: `ASK_AUNOVA_DATASET_DIR` (default `data/palm-vista`) and
//! `ASK_AUNOVA_KEYS_DIR` (default `keys`), both relative to the directory the
//! server is started from. The bind address is not configurable.

use std::{process::ExitCode, sync::Arc};

use ask_aunova_server::{
    MCP_PATH, SPOKEN_PROTOCOL_VERSION, StartupError, auth::TokenRegistry, handler::AskAunovaServer,
    listen_addr, port_from_env, router, verify::DatasetConfig,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error}");
            let mut cause = std::error::Error::source(&error);
            while let Some(reason) = cause {
                tracing::error!("  caused by: {reason}");
                cause = reason.source();
            }
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), StartupError> {
    let addr = listen_addr(port_from_env()?);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| StartupError::Bind { addr, source })?;
    // Re-read the address from the socket: with port 0 the bound port is the
    // only one worth printing.
    let bound = listener
        .local_addr()
        .map_err(|source| StartupError::Bind { addr, source })?;

    let dataset = DatasetConfig::from_env();
    tracing::info!(
        "Ask Aunova MCP server listening on http://{bound}{MCP_PATH} \
         (loopback only, MCP {SPOKEN_PROTOCOL_VERSION}, Streamable HTTP, bearer token required); \
         answering from the signed dataset at {}, re-verified on every call",
        dataset.dataset_dir().display(),
    );

    let server = AskAunovaServer::new(dataset);
    axum::serve(
        listener,
        router(Arc::new(TokenRegistry::from_env()), server),
    )
    .await
    .map_err(|source| StartupError::Serve {
        addr: bound,
        source,
    })
}
