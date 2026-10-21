//! The MCP server behaviour: what it says it is, what it declares, and what it
//! does when a tool is called or a resource is read.

use std::{borrow::Cow, sync::Arc};

use axum::http::request::Parts;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, ListResourcesResult,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
        ReadResourceResponse, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
        ServerConfig,
    },
    service::RequestContext,
};

use crate::{
    auth::Role,
    card, tools,
    verify::{DatasetConfig, DatasetUnavailable, VerifiedDataset},
};

/// The MCP revision this server speaks.
///
/// rmcp 3.5.1 knows one revision beyond this (`2026-07-28`, which replaces the
/// `initialize` handshake with per-request `_meta`), and its
/// `ProtocolVersion::LATEST` points there. This server implements the
/// `2025-11-25` handshake and says so: [`AskAunovaServer::supported_protocol_versions`]
/// narrows the advertised set to the revisions at or below this one.
pub const SPOKEN_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_11_25;

/// What the server tells a client it is for, verbatim in the `initialize` reply.
const INSTRUCTIONS: &str = "\
Ask Aunova answers questions about one building project and proves every answer. Every fact it \
states cites the source document that evidences it, and the signature over that document is \
re-checked during the call that states the fact: hashes are recomputed and Ed25519 signatures \
verified against the issuer keys on record, every time, with nothing cached. A fact it cannot \
evidence is not returned at all; a document whose bytes have moved is reported as \
signature_invalid, never as sound. If the project record itself does not verify, every tool \
refuses. What you are allowed to hear is resolved from your bearer token, server-side, and is \
never read from anything you send. This server does not read sentences: you turn what the \
person said into typed arguments — a subject, one named field, one stated value — and it \
proves that against the signed record or refuses. `verify_claim` answers SUPPORTED, \
CONTRADICTED or UNVERIFIED and never guesses at a figure written in any other form than the \
record's own integer minor units. `ask_project` is a deterministic composer over the signed \
record, not a language model: you name the topic, it answers from record fields with \
citations, or says it cannot verify. An argument outside a tool's closed set is a JSON-RPC \
error, not a verdict. Each citation is also served as an HTML evidence card under a ui:// \
resource. All of \
the project data is synthetic demo data about a fictional project; no real project, regulator \
or authority is involved, named or implied.";

/// The Ask Aunova MCP server.
///
/// Cloned once per MCP session by the Streamable HTTP transport. The dataset
/// configuration is the only thing it holds, and it sits behind an [`Arc`] so
/// every clone reads the same one. There is deliberately no per-caller state:
/// `whats_changed` hands the caller a cursor instead, so nothing this server
/// knows can be lost when a session, a connection or the process ends.
#[derive(Clone, Debug)]
pub struct AskAunovaServer {
    dataset: Arc<DatasetConfig>,
}

impl AskAunovaServer {
    /// A server reading the dataset `config` points at.
    pub fn new(config: DatasetConfig) -> Self {
        Self {
            dataset: Arc::new(config),
        }
    }

    /// Verify the dataset for this request, on a blocking thread.
    ///
    /// The verification is file I/O over a dozen small files. It is dispatched
    /// off the async runtime rather than run inside the handler, so a slow or
    /// stalled filesystem cannot block the reactor that other requests share.
    async fn verified(&self) -> Result<VerifiedDataset, DatasetUnavailable> {
        let config = Arc::clone(&self.dataset);
        match tokio::task::spawn_blocking(move || config.load_and_verify()).await {
            Ok(outcome) => outcome,
            Err(join_error) => Err(DatasetUnavailable::DidNotFinish {
                detail: join_error.to_string(),
            }),
        }
    }
}

impl ServerHandler for AskAunovaServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(SPOKEN_PROTOCOL_VERSION)
        .with_server_info(Implementation::new("ask-aunova", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    /// Every revision up to and including the one this server implements.
    ///
    /// This override does **not** stop `initialize` from agreeing to a newer
    /// revision: rmcp answers the handshake with its newest revision that still
    /// has one, whether or not this is overridden, so a client asking for
    /// `2026-07-28` is answered `2025-11-25` either way
    /// (`.qa/vendor/rmcp-3.5.1/src/service/server.rs:486-497`).
    ///
    /// What the override changes is the two places the list itself is read:
    /// `server/discover` advertises exactly these revisions
    /// (`src/handler/server.rs:392-399`), and a request that names a revision
    /// in its own `_meta` is refused with `-32022` when it is not one of them
    /// (`src/handler/server.rs:64-71`). Both are asserted over real HTTP in
    /// `tests/mcp_over_http.rs`.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(&SPOKEN_PROTOCOL_VERSION))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools::declared_tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Resolve the caller first, and fail the call if they cannot be
        // identified: a tool that answered without knowing who asked would be
        // the bug this server exists to prevent.
        let role = role_of(&context)?;
        let tool = tools::DeclaredTool::from_name(&request.name).ok_or_else(|| {
            ErrorData::invalid_params(
                format!(
                    "unknown tool `{}`; this server declares {}",
                    request.name,
                    tools::DeclaredTool::ALL
                        .map(tools::DeclaredTool::name)
                        .join(", "),
                ),
                None,
            )
        })?;

        let dataset = match self.verified().await {
            Ok(dataset) => dataset,
            Err(reason) => {
                tracing::warn!(tool = tool.name(), %reason, "refused a tool call");
                return Ok(tools::refusal(tool, &role, &reason).into());
            }
        };

        tracing::info!(
            tool = tool.name(),
            role = %role,
            verified = dataset.verified,
            declared = dataset.declared,
            "answering from a dataset verified in this call",
        );
        Ok(tools::call(tool, request.arguments.as_ref(), &role, &dataset)?.into())
    }

    /// One evidence card per document this caller may read.
    ///
    /// The list is role-filtered for the same reason every answer is: a buyer
    /// must not learn that an internal document exists by seeing its card in a
    /// directory listing.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let role = role_of(&context)?;
        let dataset = self
            .verified()
            .await
            .map_err(|reason| unavailable(&role, &reason))?;

        let resources = dataset
            .documents
            .values()
            .filter(|evidence| evidence.visibility.visible_to(&role))
            .map(|evidence| {
                Resource::new(card::card_uri(&evidence.doc_id), evidence.doc_id.clone())
                    .with_title(evidence.title.clone())
                    .with_description(format!(
                        "Evidence card for {}: issuer, date, content hash and the result of \
                         re-checking its signature when the card is read.",
                        evidence.doc_id,
                    ))
                    .with_mime_type(card::EVIDENCE_CARD_MIME_TYPE)
            })
            .collect();

        Ok(ListResourcesResult::with_all_items(resources))
    }

    /// Render one evidence card, verifying the document as the card is built.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let role = role_of(&context)?;
        let doc_id = card::doc_id_from_uri(&request.uri)
            .ok_or_else(|| unknown_resource(&request.uri))?
            .to_owned();
        let dataset = self
            .verified()
            .await
            .map_err(|reason| unavailable(&role, &reason))?;

        let evidence = dataset
            .document(&doc_id)
            .filter(|evidence| evidence.visibility.visible_to(&role))
            .ok_or_else(|| unknown_resource(&request.uri))?;

        let html = card::render(
            evidence,
            dataset.record.document_date(&doc_id),
            &dataset.record.project.name,
        );
        tracing::info!(
            doc_id = %doc_id,
            role = %role,
            status = evidence.status.as_str(),
            "served an evidence card",
        );

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(html, request.uri.clone())
                .with_mime_type(card::EVIDENCE_CARD_MIME_TYPE),
        ])
        .into())
    }
}

/// The same answer for a URI that is not a card and for a card this caller may
/// not read, so a listing refusal cannot be used to enumerate what exists.
fn unknown_resource(uri: &str) -> ErrorData {
    ErrorData::resource_not_found(
        format!(
            "no resource `{uri}` is available to you; this server serves evidence cards under \
             `{}<doc_id>`, and `resources/list` shows the ones you may read",
            card::EVIDENCE_CARD_URI_PREFIX,
        ),
        None,
    )
}

/// A resource request that cannot be served because the record did not verify.
///
/// Role-filtered for the same reason the tool refusal is: a dataset-level
/// fault names the file that broke the dataset, and that file may be one this
/// caller may not read. Both resource methods come through here.
fn unavailable(role: &Role, reason: &DatasetUnavailable) -> ErrorData {
    tracing::warn!(%role, %reason, "refused a resource read");
    ErrorData::internal_error(
        format!(
            "{}. Evidence cards are rendered from the signed dataset, so none can be served \
             while it is not one this server will answer from.",
            reason.message_for(role),
        ),
        None,
    )
}

/// Read the role the bearer-token middleware resolved.
///
/// This is the hand-off: the middleware puts the [`Role`] into the HTTP
/// request extensions, rmcp's Streamable HTTP transport injects the HTTP
/// [`Parts`] into the MCP request context, and a tool reads it back here. The
/// client has no way to reach into any of those three places.
fn role_of(context: &RequestContext<RoleServer>) -> Result<Role, ErrorData> {
    let parts = context.extensions.get::<Parts>().ok_or_else(|| {
        ErrorData::internal_error(
            "this request carries no HTTP request parts, so no caller could be identified; \
             the Ask Aunova server is reachable only over Streamable HTTP on /mcp",
            None,
        )
    })?;

    parts.extensions.get::<Role>().cloned().ok_or_else(|| {
        ErrorData::internal_error(
            "no role was resolved for this request; the bearer-token layer must run in front \
             of the MCP service and must reject any request it cannot resolve",
            None,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> AskAunovaServer {
        AskAunovaServer::new(DatasetConfig::from_lookup(|_| None))
    }

    #[test]
    fn the_server_announces_the_revision_it_implements() {
        let info = server().get_info();
        assert_eq!(info.protocol_version.as_str(), "2025-11-25");
        assert!(
            info.capabilities.tools.is_some(),
            "a server that declares tools must advertise the tools capability",
        );
        assert!(
            info.capabilities.resources.is_some(),
            "a server that serves evidence cards must advertise the resources capability",
        );
    }

    #[test]
    fn the_server_does_not_advertise_a_revision_it_cannot_speak() {
        let supported = server().supported_protocol_versions();
        assert!(supported.contains(&ProtocolVersion::V_2025_11_25));
        assert!(
            !supported.contains(&ProtocolVersion::V_2026_07_28),
            "2026-07-28 replaces the initialize handshake; this server does not implement it",
        );
    }

    #[test]
    fn the_instructions_describe_the_build_that_is_running() {
        let info = server().get_info();
        let instructions = info
            .instructions
            .as_deref()
            .expect("the server states what it is for");
        for stale in ["not-implemented", "does not answer from the project record"] {
            assert!(
                !instructions.contains(stale),
                "the instructions still describe the Day 1 build: {instructions}",
            );
        }
        assert!(
            instructions.contains("synthetic demo data"),
            "{instructions}",
        );
        assert!(
            instructions.contains("re-checked during the call"),
            "{instructions}",
        );
    }
}
