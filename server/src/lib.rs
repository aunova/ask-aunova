#![forbid(unsafe_code)]
//! Ask Aunova MCP server.
//!
//! One HTTP route, `/mcp`, serving MCP over Streamable HTTP and answering
//! `initialize` with protocol version `2025-11-25`. Every request must carry a
//! bearer token; the token is mapped to a role server-side and the role travels
//! with the request all the way into the tool handler.
//!
//! Every tool call and every resource read re-verifies the signed dataset
//! first: hashes recomputed, signatures re-checked, nothing cached. See
//! [`verify`].
//!
//! The server binds loopback and nothing else. The port is configurable; the
//! address is not.

pub mod answers;
pub mod assertion;
pub mod auth;
pub mod card;
pub mod cursor;
pub mod handler;
pub mod record;
pub mod tools;
pub mod verify;

use std::{
    net::{Ipv4Addr, SocketAddr},
    num::ParseIntError,
    sync::Arc,
};

use axum::Router;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

use crate::{auth::TokenRegistry, handler::AskAunovaServer};

pub use crate::handler::SPOKEN_PROTOCOL_VERSION;

/// The single path this server serves.
pub const MCP_PATH: &str = "/mcp";
/// Environment variable overriding [`DEFAULT_PORT`].
pub const PORT_ENV: &str = "ASK_AUNOVA_PORT";
/// Port used when [`PORT_ENV`] is unset.
pub const DEFAULT_PORT: u16 = 8731;
/// Largest `POST` body accepted on `/mcp`.
///
/// An MCP request body here is a few hundred bytes of JSON-RPC; 256 KiB leaves
/// room for a long `ask_project` question and still bounds what an unauthorized
/// caller can make the process read.
pub const MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;

/// Something that stopped the server from starting or from staying up.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// [`PORT_ENV`] was set to something that is not a port number.
    #[error(
        "{PORT_ENV} must be a TCP port number from 0 to 65535 (0 lets the OS pick one), got `{value}`"
    )]
    InvalidPort {
        /// The value as it was found in the environment.
        value: String,
        /// Why it would not parse.
        source: ParseIntError,
    },
    /// [`PORT_ENV`] was set to bytes that are not UTF-8.
    #[error("{PORT_ENV} is not valid UTF-8; it must be a TCP port number from 0 to 65535")]
    PortNotUtf8,
    /// The loopback address could not be bound.
    #[error(
        "could not bind the Ask Aunova MCP server to {addr} (this server binds loopback only; set {PORT_ENV} to use a free port)"
    )]
    Bind {
        /// The address that was attempted.
        addr: SocketAddr,
        /// The operating system's reason.
        source: std::io::Error,
    },
    /// The server stopped serving after it had bound.
    #[error("the Ask Aunova MCP server stopped serving {addr}")]
    Serve {
        /// The address it had been serving.
        addr: SocketAddr,
        /// The operating system's reason.
        source: std::io::Error,
    },
}

/// The address this server binds for `port`.
///
/// Loopback is not a default here, it is the whole policy: this server holds a
/// demo dataset and two demo identities and has no business being reachable
/// from the network.
pub fn listen_addr(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// Read the port from [`PORT_ENV`], falling back to [`DEFAULT_PORT`].
pub fn port_from_env() -> Result<u16, StartupError> {
    match std::env::var(PORT_ENV) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|source| StartupError::InvalidPort { value, source }),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_PORT),
        Err(std::env::VarError::NotUnicode(_)) => Err(StartupError::PortNotUtf8),
    }
}

/// Build the HTTP service: `/mcp` behind the bearer-token layer.
///
/// The layer wraps the route, so an unauthenticated request is answered with
/// `401` before the MCP transport, the session manager or any tool sees it.
///
/// One [`AskAunovaServer`] is shared by every MCP session; it holds the
/// dataset configuration and nothing else. There is no per-caller state
/// anywhere in this server — `whats_changed` hands the caller a cursor and
/// reads it back — so nothing here has to outlive a session, and nothing is
/// lost when one ends.
pub fn router(tokens: Arc<TokenRegistry>, server: AskAunovaServer) -> Router {
    let mcp = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_max_request_body_bytes(MAX_REQUEST_BODY_BYTES),
    );

    Router::new()
        .route_service(MCP_PATH, mcp)
        .layer(axum::middleware::from_fn(move |request, next| {
            let tokens = Arc::clone(&tokens);
            async move { auth::require_bearer_token(&tokens, request, next).await }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_binds_loopback_whatever_the_port() {
        for port in [DEFAULT_PORT, 0, 65535] {
            let addr = listen_addr(port);
            assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
            assert!(addr.is_ipv4(), "{addr} is not the IPv4 loopback");
            assert_eq!(addr.port(), port);
        }
    }
}
