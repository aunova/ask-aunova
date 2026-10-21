#![forbid(unsafe_code)]
//! The voice-assistant simulator: a real MCP client with a page in front of it.
//!
//! This crate is **not** an assistant device and does not pretend to be one.
//! It is a web page, served by this binary on loopback, that performs the real
//! MCP exchange — `initialize`, `tools/list`, `tools/call`, `resources/read` —
//! against the Ask Aunova MCP server, and shows every one of those calls while
//! it does it. The page is labelled a simulator because that is what it is.
//!
//! Three addresses matter, and all three are loopback:
//!
//! 1. the browser talks to this binary, on [`PORT_ENV`] (default
//!    [`DEFAULT_PORT`]);
//! 2. this binary talks to the MCP server, at [`MCP_ENDPOINT_ENV`] (default
//!    [`DEFAULT_MCP_ENDPOINT`]);
//! 3. nothing talks to anything else. [`endpoint::Endpoint`] refuses a
//!    non-loopback host outright, and the page carries no remote reference of
//!    any kind.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::endpoint::{Endpoint, EndpointError};

#[cfg(feature = "live")]
pub mod aws;
pub mod endpoint;
pub mod identity;
pub mod mcp;
pub mod model;
pub mod money;
pub mod page;
pub mod stub;
pub mod transcribe;
pub mod turn;
pub mod unit_id;
pub mod voice;
pub mod web;

/// Environment variable naming the MCP endpoint to talk to.
pub const MCP_ENDPOINT_ENV: &str = "ASK_AUNOVA_MCP_URL";

/// The MCP endpoint used when [`MCP_ENDPOINT_ENV`] is unset: the address the
/// server prints when it is started with its own defaults.
pub const DEFAULT_MCP_ENDPOINT: &str = "http://127.0.0.1:8731/mcp";

/// Environment variable naming the port this binary serves the page on.
pub const PORT_ENV: &str = "ASK_AUNOVA_SIMULATOR_PORT";

/// The port the page is served on when [`PORT_ENV`] is unset.
///
/// One above the MCP server's default, so running both with no configuration
/// at all works.
pub const DEFAULT_PORT: u16 = 8732;

/// Why the simulator could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// [`MCP_ENDPOINT_ENV`] did not name an endpoint this client may use.
    #[error("{MCP_ENDPOINT_ENV} is not an endpoint this client can use: {source}")]
    Endpoint {
        /// What was wrong with it.
        #[source]
        source: EndpointError,
    },
    /// [`PORT_ENV`] was not a port number.
    #[error("{PORT_ENV} must be a TCP port from 1 to 65535, not `{raw}`")]
    Port {
        /// What the variable held.
        raw: String,
    },
    /// The page's own port could not be bound.
    #[error("the simulator could not bind {addr}: {source}")]
    Bind {
        /// The address that was tried.
        addr: SocketAddr,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// The page's own server stopped with an error.
    #[error("the simulator stopped serving {addr}: {source}")]
    Serve {
        /// The address it had been serving.
        addr: SocketAddr,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },
    /// `--live` was given, but this binary cannot make a live call.
    ///
    /// Two switches guard the hosted services: the `live` cargo feature,
    /// which decides whether the code is compiled at all, and this flag,
    /// which decides whether it is used. Reaching here means the second was
    /// given without the first.
    #[error(
        "`--live` needs a binary built with the `live` feature; this one was not, so it can only \
         run the stub model, the stub transcriber and the silent voice"
    )]
    LiveNotCompiledIn,
    /// A hosted service was asked for without the configuration it needs.
    #[error("`--live` needs an AWS profile: {detail}")]
    LiveNotConfigured {
        /// What is missing, in this crate's words rather than the SDK's.
        detail: String,
    },
    /// `--live-transcribe` was given without `--live`.
    ///
    /// Serving stubs would be a silent no-op on a switch about where
    /// somebody's voice goes, which is the last switch that should fail
    /// quietly.
    #[error(
        "`--live-transcribe` sends a recording to a hosted service, and only has meaning \
         alongside `--live`; nothing was started"
    )]
    TranscribeWithoutLive,
}

/// The MCP endpoint to talk to, from the environment or the default.
pub fn endpoint_from_env() -> Result<Endpoint, StartupError> {
    let raw = match std::env::var(MCP_ENDPOINT_ENV) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => DEFAULT_MCP_ENDPOINT.to_owned(),
    };
    Endpoint::parse(&raw).map_err(|source| StartupError::Endpoint { source })
}

/// The port to serve the page on, from the environment or the default.
pub fn port_from_env() -> Result<u16, StartupError> {
    let Ok(raw) = std::env::var(PORT_ENV) else {
        return Ok(DEFAULT_PORT);
    };
    if raw.trim().is_empty() {
        return Ok(DEFAULT_PORT);
    }
    // Port 0 is permitted: it asks the operating system for a free port, which
    // is how a test starts one simulator per test without colliding.
    raw.trim()
        .parse::<u16>()
        .map_err(|_| StartupError::Port { raw })
}

/// The address the page is served on.
///
/// Loopback, and not configurable. This process holds two demo bearer tokens
/// and answers questions with them; a bind address a stray environment
/// variable could widen to `0.0.0.0` is not a knob worth having.
pub fn listen_addr(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_is_only_ever_served_on_loopback() {
        assert!(listen_addr(DEFAULT_PORT).ip().is_loopback());
        assert_eq!(listen_addr(0).port(), 0);
    }

    #[test]
    fn the_default_endpoint_is_the_server_s_own_default() {
        let endpoint = Endpoint::parse(DEFAULT_MCP_ENDPOINT).expect("the default is loopback");
        assert_eq!(endpoint.url(), "http://127.0.0.1:8731/mcp");
    }

    #[test]
    fn the_two_default_ports_do_not_collide() {
        assert_ne!(
            DEFAULT_PORT.to_string(),
            "8731",
            "the simulator must not try to bind the MCP server's own port",
        );
    }
}
