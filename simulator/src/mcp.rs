//! The MCP client: a real one, over Streamable HTTP.
//!
//! Every question opens a session, performs the MCP `initialize` handshake,
//! reads `tools/list`, issues `tools/call` for each tool the loop chooses, and
//! reads `resources/read` for the evidence cards. None of it is hand-rolled
//! over raw HTTP: it is `rmcp`'s client service on `rmcp`'s Streamable HTTP
//! client transport, which is the same protocol implementation the server side
//! of this repository uses.
//!
//! The bearer token is set in exactly one place — [`McpSession::connect`] —
//! and the identity that produced it is not carried any further. Nothing below
//! this module knows which token it is talking with, which is the point: the
//! server decides what may be answered, and this client has no way to
//! influence it.
//!
//! No transport error is ever rendered into a message here. `reqwest::Error`'s
//! `Display` embeds the request URL it was given, and `rmcp` passes that
//! through its own `Display` (`StreamableHttpError::Client(E)` is
//! `"Client error: {0}"`), so a formatted transport error is a URL in a log
//! line. Failures are classified instead, and named against the endpoint this
//! crate validated itself.

use std::time::Duration;

use rmcp::{
    RoleClient, ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
        JsonObject, ProtocolVersion, ReadResourceRequestParams, ResourceContents,
    },
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;

use crate::{endpoint::Endpoint, identity::Identity, model::ToolSpec};

/// The MCP revision this client asks for. The server implements it.
pub const SPOKEN_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_11_25;

/// Prefix of every evidence-card resource URI the server serves.
pub const EVIDENCE_CARD_URI_PREFIX: &str = "ui://ask-aunova/evidence/";

/// How long any one MCP request may take before it is abandoned.
///
/// The server verifies the whole signed dataset on every call, so this is not
/// tight; it exists because a request with no deadline is a hung page.
const REQUEST_DEADLINE: Duration = Duration::from_secs(20);

/// How long the `initialize` handshake may take.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);

/// Largest evidence card this client will hold in memory.
///
/// The cards the server renders are a few kilobytes. A card is read straight
/// into the page, so it is capped here rather than trusted.
const MAX_CARD_BYTES: usize = 256 * 1024;

/// Something that stopped an MCP exchange.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// The handshake did not complete.
    #[error("the MCP server at {endpoint} did not complete the MCP initialize handshake: {detail}")]
    Initialize {
        /// The endpoint that was tried.
        endpoint: String,
        /// Which kind of failure it was, classified rather than formatted.
        detail: &'static str,
    },
    /// The server answered the handshake with a JSON-RPC error.
    #[error(
        "the MCP server at {endpoint} refused the MCP initialize handshake ({code}): {message}"
    )]
    InitializeRefused {
        /// The endpoint that was tried.
        endpoint: String,
        /// The JSON-RPC error code.
        code: i32,
        /// The server's own message, which carries no URL.
        message: String,
    },
    /// The request did not reach the server, or its answer did not come back.
    #[error("`{operation}` against the MCP server at {endpoint} did not complete: {detail}")]
    Transport {
        /// What was being attempted.
        operation: String,
        /// The endpoint it was attempted against.
        endpoint: String,
        /// Which kind of failure it was, classified rather than formatted.
        detail: &'static str,
    },
    /// The server answered with a JSON-RPC error.
    ///
    /// This is not a refusal: a refusal is a tool *result* with `isError`, so
    /// it reaches the person who asked. This is the client getting the request
    /// wrong.
    #[error("`{operation}` was rejected by the MCP server ({code}): {message}")]
    Rejected {
        /// What was being attempted.
        operation: String,
        /// The JSON-RPC error code.
        code: i32,
        /// The server's own message, which carries no URL.
        message: String,
    },
    /// A tool answered with no structured content for the loop to read.
    #[error("`{tool}` returned no structured content, so there is nothing to answer from")]
    NoStructuredContent {
        /// The tool that was called.
        tool: String,
    },
    /// An evidence card came back as something other than text.
    #[error("evidence card `{uri}` came back without any text to render")]
    CardNotText {
        /// The resource that was read.
        uri: String,
    },
    /// An evidence card was larger than this client will hold.
    #[error("evidence card `{uri}` is larger than the {MAX_CARD_BYTES} byte cap this client reads")]
    CardTooLarge {
        /// The resource that was read.
        uri: String,
    },
}

/// What the server said it was, in the `initialize` reply.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ServerIdentity {
    /// The server's own name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// The protocol revision the handshake settled on.
    pub protocol_version: String,
}

/// One tool call and what came back.
#[derive(Clone, Debug)]
pub struct CallResult {
    /// The structured result the tool returned.
    pub result: Value,
    /// Whether the server marked it an error — a refusal, or a request this
    /// client got wrong.
    pub refused: bool,
}

/// One MCP session: initialized, and good for as many calls as the turn needs.
pub struct McpSession {
    client: RunningService<RoleClient, ClientConfig>,
    endpoint: Endpoint,
    server: ServerIdentity,
}

impl McpSession {
    /// Connect to `endpoint` and perform the MCP `initialize` handshake,
    /// authenticating as `identity`.
    ///
    /// This is the only place a bearer token is read, and the only place the
    /// identity is used. Everything the session can do afterwards is the same
    /// code whichever identity opened it.
    pub async fn connect(endpoint: &Endpoint, identity: Identity) -> Result<Self, McpError> {
        let config = StreamableHttpClientTransportConfig::with_uri(endpoint.url().to_owned())
            .auth_header(identity.token());
        let transport = StreamableHttpClientTransport::with_client(http_client(endpoint)?, config);

        let client_config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(SPOKEN_PROTOCOL_VERSION);

        let client =
            match tokio::time::timeout(HANDSHAKE_DEADLINE, client_config.serve(transport)).await {
                Err(_elapsed) => {
                    return Err(McpError::Initialize {
                        endpoint: endpoint.url().to_owned(),
                        detail: "it did not answer within the handshake deadline",
                    });
                }
                Ok(Ok(client)) => client,
                Ok(Err(error)) => return Err(initialize_failure(endpoint, error)),
            };

        // `server_info` is optional in the handshake result. A server that
        // declines to name itself is not one this page can honestly caption,
        // so it is refused rather than captioned with a blank.
        let server = client
            .peer()
            .peer_info()
            .and_then(|info| {
                info.server_info
                    .as_ref()
                    .map(|implementation| ServerIdentity {
                        name: implementation.name.clone(),
                        version: implementation.version.clone(),
                        protocol_version: info.protocol_version.to_string(),
                    })
            })
            .ok_or_else(|| McpError::Initialize {
                endpoint: endpoint.url().to_owned(),
                detail: "it completed the handshake without identifying itself",
            })?;

        Ok(Self {
            client,
            endpoint: endpoint.clone(),
            server,
        })
    }

    /// What the server said it was in the handshake.
    pub fn server(&self) -> &ServerIdentity {
        &self.server
    }

    /// The tools the server advertises, from a real `tools/list` request.
    pub async fn list_tools(&self) -> Result<Vec<ToolSpec>, McpError> {
        let tools = self
            .bounded("tools/list", self.client.list_all_tools())
            .await?;
        Ok(tools
            .into_iter()
            .map(|tool| ToolSpec {
                name: tool.name.to_string(),
                description: tool.description.as_deref().unwrap_or_default().to_owned(),
                input_schema: Value::Object((*tool.input_schema).clone()),
            })
            .collect())
    }

    /// Call one tool, and return its structured result.
    ///
    /// A refusal is not an error here. The server answers a refusal as a tool
    /// result with `isError` set precisely so the reason reaches the person
    /// who asked, and the loop above states it as a refusal.
    pub async fn call_tool(
        &self,
        tool: &str,
        arguments: JsonObject,
    ) -> Result<CallResult, McpError> {
        // `CallToolRequestParams` is `#[non_exhaustive]`, so it is built
        // through its own constructor rather than a struct literal: the
        // fields this client does not set are MRTR retry state it never
        // sends.
        let request = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        let result: CallToolResult = self
            .bounded(
                &format!("tools/call {tool}"),
                self.client.call_tool(request),
            )
            .await?;

        let structured =
            result
                .structured_content
                .ok_or_else(|| McpError::NoStructuredContent {
                    tool: tool.to_owned(),
                })?;
        Ok(CallResult {
            result: structured,
            refused: result.is_error.unwrap_or(false),
        })
    }

    /// Read one evidence card, as the HTML the server rendered for it.
    pub async fn read_evidence_card(&self, doc_id: &str) -> Result<String, McpError> {
        let uri = format!("{EVIDENCE_CARD_URI_PREFIX}{doc_id}");
        let result = self
            .bounded(
                &format!("resources/read {uri}"),
                self.client
                    .read_resource(ReadResourceRequestParams::new(uri.clone())),
            )
            .await?;

        let html = result
            .contents
            .into_iter()
            .find_map(|content| match content {
                ResourceContents::TextResourceContents { text, .. } => Some(text),
                // A blob is not a card this page can render, and neither is
                // whatever a later revision adds: both are "no text here".
                _ => None,
            })
            .ok_or_else(|| McpError::CardNotText { uri: uri.clone() })?;
        if html.len() > MAX_CARD_BYTES {
            return Err(McpError::CardTooLarge { uri });
        }
        Ok(html)
    }

    /// Close the session, letting the server release it.
    pub async fn close(self) {
        // A session that will not close is not a reason to fail an answer the
        // caller already has: the transport is dropped either way. `cancel`
        // reports how the service task ended, so the only failure it can
        // report is that task panicking, which carries no transport detail
        // and so no URL.
        match self.client.cancel().await {
            Ok(reason) => tracing::debug!(
                endpoint = self.endpoint.url(),
                ?reason,
                "closed the MCP session",
            ),
            Err(_join_error) => tracing::debug!(
                endpoint = self.endpoint.url(),
                "the MCP session task did not end cleanly",
            ),
        }
    }

    /// Run one MCP request under a deadline, classifying whatever comes back.
    async fn bounded<T>(
        &self,
        operation: &str,
        request: impl Future<Output = Result<T, ServiceError>>,
    ) -> Result<T, McpError> {
        match tokio::time::timeout(REQUEST_DEADLINE, request).await {
            Err(_elapsed) => Err(McpError::Transport {
                operation: operation.to_owned(),
                endpoint: self.endpoint.url().to_owned(),
                detail: "it did not answer within the request deadline",
            }),
            Ok(Ok(value)) => Ok(value),
            Ok(Err(ServiceError::McpError(error))) => Err(McpError::Rejected {
                operation: operation.to_owned(),
                code: error.code.0,
                message: error.message.to_string(),
            }),
            Ok(Err(error)) => Err(McpError::Transport {
                operation: operation.to_owned(),
                endpoint: self.endpoint.url().to_owned(),
                detail: service_failure(&error),
            }),
        }
    }
}

/// The HTTP client every session runs on.
///
/// Built here rather than taken from the transport's default for one reason:
/// **`no_proxy()`**. `reqwest` reads `HTTP_PROXY` and `HTTPS_PROXY` out of
/// the environment unless told not to, and every request this client makes
/// carries a bearer token in an `Authorization` header. A proxy variable set
/// anywhere in the environment — a shell profile, a terminal, a CI runner —
/// would hand those credentials to whatever host it names. This client speaks
/// to loopback and refuses anything else ([`Endpoint::parse`]), so there is no
/// legitimate proxy for it to honour. Anyone building a client here again:
/// keep this call, or the token leaves the machine.
///
/// The other two settings are the transport's own defaults, kept because
/// dropping them would change behaviour this client was tested with: no idle
/// connection pooling (a Delayed-ACK stall on Linux), and no redirects (a
/// redirect would replay the `Authorization` header at the target).
fn http_client(endpoint: &Endpoint) -> Result<reqwest::Client, McpError> {
    reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        // Not `{error}`: a `reqwest::Error`'s `Display` embeds the request
        // URL it was given, so it is classified like every other one here.
        .map_err(|_build_error| McpError::Initialize {
            endpoint: endpoint.url().to_owned(),
            detail: "this client could not be built",
        })
}

/// Classify a service failure into one fixed phrase.
///
/// Deliberately not `{error}`: `ServiceError::TransportSend` renders the
/// transport error, which for the reqwest client renders the request URL.
fn service_failure(error: &ServiceError) -> &'static str {
    match error {
        ServiceError::McpError(_) => "the server answered with a JSON-RPC error",
        ServiceError::TransportSend(_) => "the request could not be sent",
        ServiceError::TransportClosed => "the connection closed",
        ServiceError::UnexpectedResponse => "the server answered with the wrong result type",
        ServiceError::Timeout { .. } => "the request timed out inside the client",
        ServiceError::Cancelled { .. } => "the request was cancelled",
        _ => "the client could not complete the request",
    }
}

/// Classify a handshake failure, keeping the server's own message when the
/// failure came from the server rather than from the transport.
fn initialize_failure(
    endpoint: &Endpoint,
    error: rmcp::service::ClientInitializeError,
) -> McpError {
    use rmcp::service::ClientInitializeError as Failure;
    let detail = match error {
        Failure::JsonRpcError(data) => {
            return McpError::InitializeRefused {
                endpoint: endpoint.url().to_owned(),
                code: data.code.0,
                message: data.message.to_string(),
            };
        }
        Failure::NoCompatibleProtocolVersion { .. } => {
            "it speaks no protocol revision this client speaks"
        }
        Failure::ConnectionClosed(_) => "the connection closed during the handshake",
        Failure::TransportError { .. } => {
            "the request could not be sent; the server may not be running, or it may have \
             rejected the bearer token"
        }
        Failure::ExpectedInitResponse(_) | Failure::ExpectedInitResult(_) => {
            "it answered the handshake with something that is not an initialize result"
        }
        _ => "the handshake did not complete",
    };
    McpError::Initialize {
        endpoint: endpoint.url().to_owned(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transport_failure_never_renders_the_inner_error() {
        // The whole reason this function exists: `ServiceError`'s own Display
        // reaches a `reqwest::Error`, whose Display embeds the request URL.
        let message = McpError::Transport {
            operation: "tools/call get_milestones".to_owned(),
            endpoint: "http://127.0.0.1:8731/mcp".to_owned(),
            detail: service_failure(&ServiceError::TransportClosed),
        }
        .to_string();
        assert_eq!(
            message,
            "`tools/call get_milestones` against the MCP server at http://127.0.0.1:8731/mcp did \
             not complete: the connection closed",
        );
    }

    #[test]
    fn every_classified_failure_is_a_fixed_phrase() {
        for error in [
            ServiceError::TransportClosed,
            ServiceError::UnexpectedResponse,
            ServiceError::Timeout {
                timeout: Duration::from_secs(1),
            },
            ServiceError::Cancelled { reason: None },
        ] {
            let phrase = service_failure(&error);
            assert!(!phrase.is_empty());
            assert!(
                !phrase.contains("http"),
                "a classified phrase must never carry a URL: {phrase}",
            );
        }
    }

    #[test]
    fn the_card_uri_is_the_one_the_server_serves() {
        assert_eq!(EVIDENCE_CARD_URI_PREFIX, "ui://ask-aunova/evidence/");
    }
}
