//! The server as a client meets it: a real socket on loopback, real HTTP,
//! real Streamable HTTP framing.
//!
//! These tests drive the same `Router` that the binary serves, so they cover
//! the bearer-token layer, the rmcp transport, the live verification run and
//! the handler together. Any test that tampers with a document does it to a
//! copy of the dataset under `TMPDIR`, never to the committed one.

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use ask_aunova_server::{
    MCP_PATH, auth::TokenRegistry, cursor, handler::AskAunovaServer, router, verify::DatasetConfig,
};
use common::Fixture;
use reqwest::{Client, Response, StatusCode, header};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

const BUYER: &str = "demo-token-buyer-1204";
const DEVELOPER: &str = "demo-token-developer";
const PROTOCOL_VERSION: &str = "2025-11-25";
const NEXT_REVISION: &str = "2026-07-28";
const ACCEPT: &str = "application/json, text/event-stream";
const STRUCTURAL_CERTIFICATE_DOC: &str =
    "docs/certificate-structural-integrity-CERT-STRUCT-2026-114.md";
const CONTRACTOR_NOTE_DOC: &str = "docs/contractor-note-CN-14.md";
/// The `doc_id` of that document, which only the developer may read.
const CONTRACTOR_NOTE_ID: &str = "CN-14";

/// A server on a loopback port the OS picked, shut down when dropped.
struct TestServer {
    url: String,
    client: Client,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    async fn start(dataset: DatasetConfig) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("read the bound address");
        let app = router(
            Arc::new(TokenRegistry::from_lookup(|_| None)),
            AskAunovaServer::new(dataset),
        );
        let task = tokio::spawn(async move {
            let _served = axum::serve(listener, app).await;
        });

        Self {
            url: format!("http://{addr}{MCP_PATH}"),
            // Generous on purpose: the field table below drives roughly a
            // thousand calls through one server while every other test in
            // this binary runs beside it, and each call re-verifies fourteen
            // documents in an unoptimised build. A timeout here would be a
            // measurement of the machine, not of the server.
            client: Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("build a plain HTTP client"),
            task,
        }
    }

    /// POST a JSON-RPC message with a bearer token, optionally inside a session.
    async fn post(&self, token: &str, session: Option<&str>, body: &Value) -> Response {
        let mut extra = vec![];
        if let Some(session) = session {
            extra.push(("Mcp-Session-Id", session.to_owned()));
            extra.push(("MCP-Protocol-Version", PROTOCOL_VERSION.to_owned()));
        }
        self.post_with_headers(token, &extra, body).await
    }

    /// POST with whatever headers the test wants to send.
    async fn post_with_headers(
        &self,
        token: &str,
        extra: &[(&str, String)],
        body: &Value,
    ) -> Response {
        let mut request = self
            .client
            .post(&self.url)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, ACCEPT)
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
        for (name, value) in extra {
            request = request.header(*name, value.clone());
        }
        request.send().await.expect("the server answered")
    }

    /// Ask for something and walk away before the answer arrives, as a client
    /// whose user closed the tab does.
    ///
    /// Everything about it is its own: its own client, its own connection and
    /// its own MCP session, so a response composed after it left cannot be
    /// delivered on a stream some later call is reading. The token is the
    /// same one, which is the whole point — it is the same caller coming
    /// back.
    async fn abandon(&self, token: &str, name: &str, arguments: &Value) {
        let leaving = Client::builder()
            .timeout(budget())
            .pool_max_idle_per_host(0)
            .build()
            .expect("build a client that gives up quickly");
        let _gave_up = leaving
            .post(&self.url)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::ACCEPT, ACCEPT)
            .header(header::CONTENT_TYPE, "application/json")
            .body(tool_call(next_id(), name, arguments).to_string())
            .send()
            .await;
    }
}

/// How long an abandoning client waits before it stops caring.
const fn budget() -> Duration {
    Duration::from_millis(1)
}

/// Read a JSON-RPC message out of either a JSON body or an SSE stream.
async fn message(response: Response) -> Value {
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response.text().await.expect("read the response body");

    if content_type.starts_with("application/json") {
        return serde_json::from_str(&body).expect("the JSON body parses");
    }
    // The transport primes the stream with a retry event and an empty data
    // line before the message itself, so take the first data line with a
    // payload in it.
    let data = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .find(|data| !data.is_empty())
        .unwrap_or_else(|| panic!("no SSE data line carried a message: {body}"));
    serde_json::from_str(data).expect("the SSE data line parses")
}

/// Run `initialize` and return the negotiated reply plus the session id.
async fn initialize(server: &TestServer, token: &str) -> (Value, Option<String>) {
    let response = server
        .post(
            token,
            None,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "ask-aunova-tests", "version": "0.1.0"}
                }
            }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (message(response).await, session)
}

/// Open a session and return its id.
async fn session_for(server: &TestServer, token: &str) -> Option<String> {
    initialize(server, token).await.1
}

/// A request id no other request in this binary uses.
///
/// Streamable HTTP may deliver a queued message on a later POST's stream, so
/// a reply has to be matched to the request that asked for it. Reusing one id
/// everywhere is how a test reads someone else's answer and passes.
fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(100);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The JSON-RPC request body for one tool call.
fn tool_call(id: u64, name: &str, arguments: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    })
}

/// Call one tool and return the whole JSON-RPC reply.
async fn call_tool(
    server: &TestServer,
    token: &str,
    session: Option<&str>,
    name: &str,
    arguments: Value,
) -> Value {
    let id = next_id();
    let response = server
        .post(token, session, &tool_call(id, name, &arguments))
        .await;
    assert_eq!(response.status(), StatusCode::OK, "tools/call `{name}`");
    let reply = message(response).await;
    assert_eq!(
        reply["id"], id,
        "this reply answers a different request than the one just sent: {reply}",
    );
    reply
}

/// The structured body of a tool result, asserting whether it is a refusal.
fn structured(reply: &Value, expect_error: bool) -> &Value {
    assert_eq!(
        reply["result"]["isError"], expect_error,
        "unexpected isError: {reply}",
    );
    let structured = &reply["result"]["structuredContent"];
    assert!(
        structured.is_object(),
        "a tool result must carry structured content: {reply}",
    );
    structured
}

/// The message of a JSON-RPC error reply, asserting there is no result at all.
fn protocol_error(reply: &Value) -> &str {
    assert!(
        reply["result"].is_null(),
        "an argument this server refuses must not also produce a result: {reply}",
    );
    reply["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON-RPC error: {reply}"))
}

/// Every string in a JSON value, for "this must not appear anywhere" checks.
fn rendered(value: &Value) -> String {
    value.to_string()
}

/// The arguments of one `verify_claim` call.
fn claim(subject_type: &str, subject_id: &str, field: &str, asserted_value: &str) -> Value {
    json!({
        "subject_type": subject_type,
        "subject_id": subject_id,
        "field": field,
        "asserted_value": asserted_value,
    })
}

/// The committed record, as raw JSON.
///
/// The field table below reads its truth from here rather than from the
/// server's own types, so the test and the thing under test arrive at the
/// record's values by two different routes.
fn committed_record() -> Value {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the server package sits inside the repository")
        .to_path_buf();
    let text = fs::read_to_string(repo.join("data/palm-vista/project.json"))
        .expect("read the committed project record");
    serde_json::from_str(&text).expect("the committed record is JSON")
}

// ---------------------------------------------------------------------------
// Handshake and declaration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_answers_with_protocol_version_2025_11_25() {
    let fixture = Fixture::new("initialize");
    let server = TestServer::start(fixture.config()).await;
    let (reply, session) = initialize(&server, BUYER).await;

    assert_eq!(
        reply["result"]["protocolVersion"], PROTOCOL_VERSION,
        "{reply}",
    );
    assert_eq!(
        reply["result"]["serverInfo"]["name"], "ask-aunova",
        "{reply}"
    );
    assert!(
        reply["result"]["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("synthetic demo data")),
        "the instructions must say the data is synthetic: {reply}",
    );
    assert!(
        session.is_some(),
        "the Streamable HTTP transport hands out a session id: {reply}",
    );
}

/// What rmcp does on its own, recorded so it is not mistaken for our pin.
///
/// `2026-07-28` replaced the `initialize` handshake, so rmcp answers it with
/// the newest revision that still has one — with or without
/// `supported_protocol_versions` overridden
/// (`.qa/vendor/rmcp-3.5.1/src/service/server.rs:486-497`). The two tests
/// below this one are the ones that depend on the override.
#[tokio::test]
async fn initialize_answers_a_newer_revision_with_the_one_we_speak() {
    let fixture = Fixture::new("initialize-newer");
    let server = TestServer::start(fixture.config()).await;

    let response = server
        .post(
            BUYER,
            None,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": NEXT_REVISION,
                    "capabilities": {},
                    "clientInfo": {"name": "ask-aunova-tests", "version": "0.1.0"}
                }
            }),
        )
        .await;
    let reply = message(response).await;
    assert_eq!(
        reply["result"]["protocolVersion"], PROTOCOL_VERSION,
        "a client asking for a revision this server does not implement is answered with one \
         it does: {reply}",
    );
}

/// The first thing the pin actually changes: what `server/discover` advertises.
#[tokio::test]
async fn server_discover_omits_the_revision_this_server_cannot_speak() {
    let fixture = Fixture::new("discover");
    let server = TestServer::start(fixture.config()).await;

    let response = server
        .post_with_headers(
            DEVELOPER,
            &[("MCP-Protocol-Version", PROTOCOL_VERSION.to_owned())],
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "server/discover",
                "params": {
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                        "io.modelcontextprotocol/clientInfo": {
                            "name": "ask-aunova-tests", "version": "0.1.0"
                        },
                        "io.modelcontextprotocol/clientCapabilities": {}
                    }
                }
            }),
        )
        .await;
    let reply = message(response).await;

    let advertised: Vec<&str> = reply["result"]["supportedVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("server/discover returned no supportedVersions: {reply}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();

    assert!(
        advertised.contains(&PROTOCOL_VERSION),
        "discovery must advertise the revision we implement: {reply}",
    );
    assert!(
        !advertised.contains(&NEXT_REVISION),
        "discovery must not advertise {NEXT_REVISION}, which this server does not implement: \
         {reply}",
    );
}

/// The second thing the pin changes: a request that names `2026-07-28` in its
/// own `_meta` is refused with `-32022`
/// (`.qa/vendor/rmcp-3.5.1/src/handler/server.rs:64-71`).
#[tokio::test]
async fn a_request_naming_a_revision_we_do_not_speak_is_refused_with_32022() {
    let fixture = Fixture::new("unsupported-revision");
    let server = TestServer::start(fixture.config()).await;

    let response = server
        .post_with_headers(
            DEVELOPER,
            &[
                // Both headers are required before the request is even looked
                // at: `_meta` naming a version needs the matching
                // `MCP-Protocol-Version` header, and a 2026-07-28 request must
                // carry the SEP-2243 `Mcp-Method` header. Without them the
                // transport answers -32020 and the version check is never
                // reached.
                ("MCP-Protocol-Version", NEXT_REVISION.to_owned()),
                ("Mcp-Method", "tools/list".to_owned()),
            ],
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/list",
                "params": {
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": NEXT_REVISION,
                        "io.modelcontextprotocol/clientInfo": {
                            "name": "ask-aunova-tests", "version": "0.1.0"
                        },
                        "io.modelcontextprotocol/clientCapabilities": {}
                    }
                }
            }),
        )
        .await;
    let reply = message(response).await;

    assert_eq!(
        reply["error"]["code"], -32022,
        "a request naming {NEXT_REVISION} must be refused as an unsupported protocol version, \
         not answered: {reply}",
    );
    assert!(
        reply["result"]["tools"].is_null(),
        "a refused request must not also return the tool list: {reply}",
    );
}

#[tokio::test]
async fn tools_list_declares_the_six_tools() {
    let fixture = Fixture::new("tools-list");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let response = server
        .post(
            BUYER,
            session.as_deref(),
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let reply = message(response).await;

    let tools = reply["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no tools array: {reply}"));
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();

    assert_eq!(
        names,
        [
            "get_milestones",
            "get_certificates",
            "get_payment_status",
            "verify_claim",
            "ask_project",
            "whats_changed",
        ],
        "tools/list must declare exactly the six tools: {reply}",
    );

    // The schemas a calling model actually reads: closed enums, and no prose.
    let declared = |name: &str| -> &Value {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("`{name}` is not declared: {reply}"))
    };
    let properties = |name: &str| -> Vec<String> {
        declared(name)["inputSchema"]["properties"]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut claim_arguments = properties("verify_claim");
    claim_arguments.sort();
    assert_eq!(
        claim_arguments,
        [
            "asserted_value",
            "field",
            "instalment_id",
            "subject_id",
            "subject_type",
        ],
        "verify_claim takes typed arguments, not a sentence: {reply}",
    );
    assert!(
        declared("verify_claim")["inputSchema"]["properties"]["subject_type"]["enum"].is_array(),
        "`subject_type` must be a closed enum in the schema: {reply}",
    );
    assert!(
        declared("ask_project")["inputSchema"]["properties"]["topic"]["enum"].is_array(),
        "`topic` must be a closed enum in the schema: {reply}",
    );
    let whole = rendered(&reply);
    for prose in ["\"claim\":{", "\"question\":{"] {
        assert!(
            !whole.contains(prose),
            "no tool may still take free text ({prose}): {whole}",
        );
    }
}

#[tokio::test]
async fn calling_an_undeclared_tool_is_a_protocol_error() {
    let fixture = Fixture::new("undeclared-tool");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, DEVELOPER).await;

    let reply = call_tool(
        &server,
        DEVELOPER,
        session.as_deref(),
        "drop_everything",
        json!({}),
    )
    .await;

    let error = protocol_error(&reply);
    assert!(
        error.contains("drop_everything") && error.contains("get_milestones"),
        "the error should name what was asked for and what exists: {error}",
    );
}

// ---------------------------------------------------------------------------
// An argument this server will not read is a schema error, never a verdict
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_argument_outside_the_schema_is_refused_without_a_verdict() {
    let fixture = Fixture::new("schema-errors");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let cases: [(&str, Value, &str); 7] = [
        (
            "verify_claim",
            claim("escrow", "ESC-PVT-4471", "status", "attested"),
            "`unit`, `certificate` or `milestone`",
        ),
        (
            "verify_claim",
            claim("unit", "1204", "vibes", "good"),
            "`total_price_minor`",
        ),
        (
            "verify_claim",
            json!({"subject_type": "unit", "subject_id": "1204", "field": "paid_to_date_minor"}),
            "`asserted_value`",
        ),
        (
            "verify_claim",
            json!({
                "subject_type": "unit", "subject_id": "1204",
                "field": "paid_to_date_minor", "asserted_value": 147_000_000
            }),
            "a number",
        ),
        (
            "ask_project",
            json!({"topic": "handover"}),
            "`overview`, `milestones`, `certificates`, `payments` or `escrow`",
        ),
        (
            "ask_project",
            json!({}),
            "non-empty string argument `topic`",
        ),
        (
            "whats_changed",
            json!({"since_cursor": "milestone:MS-07"}),
            "cannot read `since_cursor`",
        ),
    ];

    for (tool, arguments, expected) in cases {
        let reply = call_tool(&server, BUYER, session.as_deref(), tool, arguments.clone()).await;
        let error = protocol_error(&reply);
        assert!(
            error.contains(expected),
            "`{tool}` {arguments} should have been refused with `{expected}`, got: {error}",
        );
        let whole = rendered(&reply);
        for verdictish in ["SUPPORTED", "CONTRADICTED", "UNVERIFIED", "verdict"] {
            assert!(
                !whole.contains(verdictish),
                "a schema error must never read like a verdict ({verdictish}): {whole}",
            );
        }
    }
}

/// R2: a name the tool does not declare is a JSON-RPC error, and the call
/// that carried it is never answered.
///
/// Every schema says `"additionalProperties": false`. The r3 review sent
/// thirteen true assertions, each with one extra name — `negated`, `claim`,
/// `currency`, `comparison`, `as_of`, `unit`, `subject_ids`, `role` — and
/// got SUPPORTED thirteen times, because the handler read the names it knew
/// and dropped the rest. A verdict that answers an assertion the client did
/// not send is worse than no verdict at all: with `"negated": true` beside
/// it, SUPPORTED reads as its own opposite. The same hole let `ask_project`
/// answer about unit 1204 when the caller had named 0802 under `unit_id`,
/// `units`, `Unit` or `question`.
///
/// The control at the end carries as much weight as the cases: the
/// identical claim *without* the extra name is SUPPORTED, so what these
/// calls are refused for is the undeclared name and not the claim.
#[tokio::test]
async fn an_argument_the_tool_does_not_declare_is_refused_and_never_answered() {
    let fixture = Fixture::new("undeclared-arguments");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    // True of the record, and true of this caller's own unit.
    let paid = claim("unit", "1204", "paid_to_date_minor", "147000000");
    let with = |extra: &[(&str, Value)]| -> Value {
        let mut arguments = paid.clone();
        for (name, value) in extra {
            arguments[*name] = value.clone();
        }
        arguments
    };

    let cases: [(&str, Value, &str); 16] = [
        (
            "verify_claim",
            with(&[("negated", json!(true))]),
            "`negated`",
        ),
        (
            "verify_claim",
            with(&[("claim", json!("unit 1204 has paid nothing at all"))]),
            "`claim`",
        ),
        (
            "verify_claim",
            with(&[("currency", json!("USD"))]),
            "`currency`",
        ),
        (
            "verify_claim",
            with(&[("comparison", json!("greater_than"))]),
            "`comparison`",
        ),
        (
            "verify_claim",
            with(&[("as_of", json!("2025-01-01"))]),
            "`as_of`",
        ),
        ("verify_claim", with(&[("unit", json!("0802"))]), "`unit`"),
        (
            "verify_claim",
            with(&[("subject_ids", json!(["0802"]))]),
            "`subject_ids`",
        ),
        (
            "verify_claim",
            with(&[("role", json!("developer"))]),
            "`role`",
        ),
        (
            "verify_claim",
            with(&[
                ("field_2", json!("status")),
                ("asserted_value_2", json!("complete")),
            ]),
            "and 1 more it does not declare",
        ),
        (
            "ask_project",
            json!({"topic": "payments", "unit_id": "0802"}),
            "`unit_id`",
        ),
        (
            "ask_project",
            json!({"topic": "payments", "units": ["0802"]}),
            "`units`",
        ),
        (
            "ask_project",
            json!({"topic": "payments", "Unit": "0802"}),
            "`Unit`",
        ),
        (
            "ask_project",
            json!({"topic": "payments", "question": "How much has unit 0802 paid?"}),
            "`question`",
        ),
        (
            "whats_changed",
            json!({"cursor": "v1.6d696c6573746f6e65"}),
            "`cursor`",
        ),
        (
            "get_milestones",
            json!({"unit": "1204"}),
            "it takes no arguments at all",
        ),
        (
            "get_payment_status",
            json!({"unit": "1204", "as_of": "2026-01-01"}),
            "`as_of`",
        ),
    ];

    for (tool, arguments, expected) in cases {
        let reply = call_tool(&server, BUYER, session.as_deref(), tool, arguments.clone()).await;
        let error = protocol_error(&reply);
        assert_eq!(
            reply["error"]["code"], -32602,
            "`{tool}` {arguments} must be an invalid-params error: {reply}",
        );
        assert!(
            error.contains("does not declare an argument") && error.contains(expected),
            "`{tool}` {arguments} should have been refused naming {expected}, got: {error}",
        );
        let whole = rendered(&reply);
        for verdictish in [
            "SUPPORTED",
            "CONTRADICTED",
            "UNVERIFIED",
            "verdict",
            "answered",
            "147000000",
        ] {
            assert!(
                !whole.contains(verdictish),
                "a call this server would not read must not read like an answer \
                 ({verdictish}): {whole}",
            );
        }
    }

    // The same claim with only the names the schema declares: SUPPORTED. So
    // the sixteen refusals above are about the name, not about the claim.
    let control = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "verify_claim",
        paid.clone(),
    )
    .await;
    assert_eq!(
        structured(&control, false)["verdict"],
        "SUPPORTED",
        "the claim itself is true of the record: {control}",
    );

    // And `ask_project`, given only the argument it declares, still refuses
    // a unit outside the token instead of substituting the caller's own.
    let refused = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "ask_project",
        json!({"topic": "payments", "unit": "0802"}),
    )
    .await;
    let body = structured(&refused, true);
    assert_eq!(body["reason"], "role_scope", "{refused}");
    assert_eq!(body["answered"], false, "{refused}");
}

// ---------------------------------------------------------------------------
// Role, resolved from the token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_buyer_is_refused_another_units_payments_and_the_developer_is_not() {
    let fixture = Fixture::new("payment-scope");
    let server = TestServer::start(fixture.config()).await;

    let buyer_session = session_for(&server, BUYER).await;
    let refused = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_payment_status",
        json!({"unit": "0802"}),
    )
    .await;
    let body = structured(&refused, true);
    assert_eq!(body["status"], "refused", "{refused}");
    assert_eq!(body["reason"], "role_scope", "{refused}");
    let leaked = rendered(&refused);
    for secret in ["19800000", "BUYER-PV-0802", "PS-0802", "overdue"] {
        assert!(
            !leaked.contains(secret),
            "the refusal leaked unit 0802's data (`{secret}`): {leaked}",
        );
    }

    // The same token reads its own unit.
    let own = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_payment_status",
        json!({"unit": "1204"}),
    )
    .await;
    let own_body = structured(&own, false);
    assert_eq!(own_body["unit"], "1204", "{own}");
    assert_eq!(own_body["paid_to_date_minor"], 147_000_000, "{own}");
    assert_eq!(own_body["outstanding_minor"], 98_000_000, "{own}");
    assert_eq!(own_body["has_overdue_instalment"], false, "{own}");
    assert!(
        own_body["citations"]
            .as_array()
            .is_some_and(|citations| citations.iter().any(|id| id == "PS-1204")),
        "a payment answer must cite the unit's signed schedule: {own}",
    );

    // The developer token reads the unit the buyer was refused.
    let developer_session = session_for(&server, DEVELOPER).await;
    let allowed = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "get_payment_status",
        json!({"unit": "0802"}),
    )
    .await;
    let allowed_body = structured(&allowed, false);
    assert_eq!(allowed_body["unit"], "0802", "{allowed}");
    assert_eq!(allowed_body["role"], "developer", "{allowed}");
    assert_eq!(allowed_body["has_overdue_instalment"], true, "{allowed}");
    assert!(
        allowed_body["instalments"]
            .as_array()
            .is_some_and(|instalments| !instalments.is_empty()),
        "the developer must get the schedule itself: {allowed}",
    );
}

/// The role is the token's, and an argument that tries to name one is not
/// read at all.
///
/// This test used to assert that `{"role": "developer", "unit": "0802"}` was
/// *dropped* and `get_milestones` answered as the buyer anyway. Dropping an
/// undeclared name is exactly the r3 review's R2, so it is no longer what
/// happens: the call is a JSON-RPC error and never reaches the record. The
/// guarantee the test was written for is asserted on the next call instead —
/// the same tool, called the way it is declared, still answers in the
/// token's role and still withholds the internal note.
#[tokio::test]
async fn the_resolved_role_follows_the_token_not_the_request() {
    let fixture = Fixture::new("role-from-token");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    // A client trying to name its own role, and to widen it.
    let reply = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "get_milestones",
        json!({"role": "developer", "unit": "0802"}),
    )
    .await;

    let error = protocol_error(&reply);
    assert!(
        error.contains("get_milestones") && error.contains("`role`"),
        "the refusal must name the tool and the argument it does not declare: {error}",
    );
    let whole = rendered(&reply);
    for echoed in ["developer", "0802"] {
        assert!(
            !whole.contains(echoed),
            "a refusal must not echo the value a caller smuggled (`{echoed}`): {whole}",
        );
    }

    // The same tool, called as declared: the role is still the token's.
    let answered = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let body = structured(&answered, false);
    assert_eq!(
        body["role"], "buyer:1204",
        "the role must come from the token, never from the arguments: {answered}",
    );
    assert!(
        !rendered(&answered).contains("CN-14"),
        "a buyer must not reach the internal contractor note: {answered}",
    );
}

#[tokio::test]
async fn milestones_are_cited_or_withheld_and_the_developer_sees_the_internal_note() {
    let fixture = Fixture::new("milestones");
    let server = TestServer::start(fixture.config()).await;

    let buyer_session = session_for(&server, BUYER).await;
    let buyer = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let buyer_body = structured(&buyer, false);

    let stated = buyer_body["milestones"]
        .as_array()
        .unwrap_or_else(|| panic!("no milestones array: {buyer}"));
    assert!(!stated.is_empty(), "{buyer}");
    for milestone in stated {
        assert!(
            milestone["citations"]
                .as_array()
                .is_some_and(|citations| !citations.is_empty()),
            "a stated milestone with no citation must not be returned: {milestone}",
        );
    }

    let withheld: Vec<&str> = buyer_body["withheld"]
        .as_array()
        .unwrap_or_else(|| panic!("no withheld array: {buyer}"))
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(
        withheld.contains(&"MS-09"),
        "a milestone the record evidences with nothing must be withheld, not stated: {buyer}",
    );
    assert!(
        withheld.contains(&"MS-10"),
        "a milestone evidenced only by a developer-only document must be withheld from a \
         buyer: {buyer}",
    );
    assert!(
        !rendered(&buyer).contains("CN-14"),
        "a buyer must never see the internal note's id: {buyer}",
    );

    let developer_session = session_for(&server, DEVELOPER).await;
    let developer = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let developer_body = structured(&developer, false);
    assert!(
        rendered(developer_body).contains("CN-14"),
        "the developer must see the contractor note behind a milestone: {developer}",
    );
    let developer_withheld: Vec<&str> = developer_body["withheld"]
        .as_array()
        .unwrap_or_else(|| panic!("no withheld array: {developer}"))
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(
        !developer_withheld.contains(&"MS-10"),
        "the developer can see MS-10's evidence, so it must not be withheld: {developer}",
    );
}

/// N-c: the counts in an answer describe the caller's own documents.
///
/// A buyer who is told "14 declared, 13 verified" has been told that four
/// documents exist which they may not read, and that one of them is broken.
#[tokio::test]
async fn the_document_counts_in_an_answer_are_the_callers_own() {
    let fixture = Fixture::new("role-counts");
    let server = TestServer::start(fixture.config()).await;
    let buyer_session = session_for(&server, BUYER).await;
    let developer_session = session_for(&server, DEVELOPER).await;

    let counts = |body: &Value| -> (u64, u64) {
        (
            body["verification"]["documents_declared"]
                .as_u64()
                .unwrap_or_else(|| panic!("no declared count: {body}")),
            body["verification"]["documents_verified"]
                .as_u64()
                .unwrap_or_else(|| panic!("no verified count: {body}")),
        )
    };

    let buyer = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    assert_eq!(
        counts(structured(&buyer, false)),
        (10, 10),
        "a buyer may read nine public documents and their own schedule: {buyer}",
    );

    let developer = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    assert_eq!(
        counts(structured(&developer, false)),
        (14, 14),
        "the developer may read every declared document: {developer}",
    );

    // Break the one document only the developer may read.
    fixture.tamper(CONTRACTOR_NOTE_DOC);

    let buyer_after = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let buyer_after_body = structured(&buyer_after, false);
    assert_eq!(
        counts(buyer_after_body),
        (10, 10),
        "a developer-only document failing must not show up in a buyer's totals: {buyer_after}",
    );
    assert!(
        !rendered(&buyer_after).contains("CN-14"),
        "a buyer must not learn the internal note exists from a failure: {buyer_after}",
    );

    let developer_after = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    assert_eq!(
        counts(structured(&developer_after, false)),
        (14, 13),
        "the developer is told their own document broke: {developer_after}",
    );
}

// ---------------------------------------------------------------------------
// Verification is live
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tampered_certificate_is_marked_signature_invalid_without_a_restart() {
    let fixture = Fixture::new("tampered-certificate");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let before = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "get_certificates",
        json!({}),
    )
    .await;
    let before_body = structured(&before, false);
    let structural = certificate_named(before_body, "CERT-STRUCT-2026-114");
    assert_eq!(structural["signature"]["status"], "verified", "{before}");
    assert_eq!(structural["may_be_relied_on"], true, "{before}");

    // One bit of one byte, while the server keeps running.
    fixture.tamper(STRUCTURAL_CERTIFICATE_DOC);

    let after = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "get_certificates",
        json!({}),
    )
    .await;
    let after_body = structured(&after, false);
    let structural = certificate_named(after_body, "CERT-STRUCT-2026-114");
    assert_eq!(
        structural["signature"]["status"], "signature_invalid",
        "a document whose bytes moved must never read as sound: {after}",
    );
    assert_eq!(structural["may_be_relied_on"], false, "{after}");
    assert!(
        structural["signature"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("hashes to")),
        "the detail must name the check that failed: {after}",
    );
}

/// The same verifier for both roles, over a different set of documents: a
/// developer-only certificate can support (or contradict) a claim for the
/// developer, and is not so much as named to a buyer.
#[tokio::test]
async fn a_developer_only_document_answers_a_claim_only_for_the_developer() {
    let fixture = Fixture::new("claim-scope");
    let server = TestServer::start(fixture.config()).await;
    let arguments = claim("certificate", "CERT-MEP-2026-052", "status", "issued");

    let developer_session = session_for(&server, DEVELOPER).await;
    let developer = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "verify_claim",
        arguments.clone(),
    )
    .await;
    let developer_body = structured(&developer, false);
    assert_eq!(
        developer_body["verdict"], "CONTRADICTED",
        "the record records that certificate as pending, not issued: {developer}",
    );
    assert_eq!(
        developer_body["subject"]["id"], "CERT-MEP-2026-052",
        "{developer}",
    );

    let buyer_session = session_for(&server, BUYER).await;
    let buyer = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "verify_claim",
        arguments,
    )
    .await;
    let buyer_body = structured(&buyer, false);
    assert_eq!(
        buyer_body["verdict"], "UNVERIFIED",
        "a buyer cannot be answered from a developer-only document: {buyer}",
    );
    assert_eq!(buyer_body["reason"], "subject_not_resolved", "{buyer}");
    assert!(
        !rendered(&buyer).contains("CERT-MEP-2026-052"),
        "a refusal must not name the document the caller may not see, even to repeat it back \
         to them: {buyer}",
    );
}

#[tokio::test]
async fn a_tampered_certificate_makes_verify_claim_answer_unverified() {
    let fixture = Fixture::new("tampered-claim");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;
    let arguments = claim("certificate", "CERT-STRUCT-2026-114", "status", "issued");

    let before = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "verify_claim",
        arguments.clone(),
    )
    .await;
    let before_body = structured(&before, false);
    assert_eq!(before_body["verdict"], "SUPPORTED", "{before}");
    assert_eq!(
        before_body["subject"]["id"], "CERT-STRUCT-2026-114",
        "{before}"
    );

    fixture.tamper(STRUCTURAL_CERTIFICATE_DOC);

    let after = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "verify_claim",
        arguments,
    )
    .await;
    let after_body = structured(&after, false);
    assert_eq!(
        after_body["verdict"], "UNVERIFIED",
        "a claim resting on a document that no longer verifies must be UNVERIFIED: {after}",
    );
    assert_eq!(after_body["reason"], "evidence_unverified", "{after}");
    assert!(
        after_body["unverified"]
            .as_array()
            .is_some_and(|entries| entries
                .iter()
                .any(|entry| entry["doc_id"] == "CERT-STRUCT-2026-114")),
        "the failed document must be named: {after}",
    );
    assert!(
        after_body["supporting_documents"]
            .as_array()
            .is_some_and(|documents| documents
                .iter()
                .all(|document| document["doc_id"] != "CERT-STRUCT-2026-114")),
        "a document that failed verification must not be listed as support: {after}",
    );
}

#[tokio::test]
async fn a_tampered_project_record_makes_every_tool_refuse() {
    let fixture = Fixture::new("tampered-record");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, DEVELOPER).await;

    let before = call_tool(
        &server,
        DEVELOPER,
        session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    structured(&before, false);

    // The on-camera edit: change a milestone's progress in the record. The
    // file is still valid JSON, and it is no longer the file that was signed.
    fixture.replace_once(
        "project.json",
        "\"percent_complete\": 48",
        "\"percent_complete\": 58",
    );

    for (tool, arguments) in every_tool_call() {
        let reply = call_tool(&server, DEVELOPER, session.as_deref(), tool, arguments).await;
        let body = structured(&reply, true);
        assert_eq!(
            body["status"], "project_record_unverified",
            "`{tool}` must refuse while the record does not verify: {reply}",
        );
        assert_eq!(body["tool"], tool, "{reply}");
        let leaked = rendered(&reply);
        for secret in ["MS-07", "CERT-STRUCT-2026-114", "48213900000"] {
            assert!(
                !leaked.contains(secret),
                "`{tool}` leaked record data (`{secret}`) while refusing: {leaked}",
            );
        }
    }
}

/// One well-formed call to each of the six tools.
fn every_tool_call() -> [(&'static str, Value); 6] {
    [
        ("get_milestones", json!({})),
        ("get_certificates", json!({})),
        ("get_payment_status", json!({"unit": "1204"})),
        (
            "verify_claim",
            claim("milestone", "MS-07", "percent_complete", "48"),
        ),
        ("ask_project", json!({"topic": "milestones"})),
        ("whats_changed", json!({})),
    ]
}

/// A record that is not JSON at all refuses too: the buyer is told the kind of
/// failure and nothing else, and the developer's copy of the detail describes
/// the fault without describing the server's filesystem.
#[tokio::test]
async fn a_record_that_stopped_being_json_refuses_without_naming_a_server_path() {
    let fixture = Fixture::new("unreadable-record");
    let server = TestServer::start(fixture.config()).await;
    let buyer_session = session_for(&server, BUYER).await;
    let developer_session = session_for(&server, DEVELOPER).await;

    fixture.tamper("project.json");

    let reply = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let body = structured(&reply, true);
    assert_eq!(body["status"], "dataset_unreadable", "{reply}");

    let message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("a refusal must say why: {reply}"));
    assert!(
        message.contains("the signed dataset could not be read"),
        "the refusal must name the kind of failure: {message}",
    );
    // A parse failure's detail quotes the text it tripped on, and in a record
    // this is one unit's figures. A buyer gets the kind, never the detail.
    for withheld in ["project.json", "at line", "column", "expected"] {
        assert!(
            !message.contains(withheld),
            "a buyer must not be told `{withheld}` about a record that would not parse: \
             {message}",
        );
    }

    let operator = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let operator_body = structured(&operator, true);
    assert_eq!(operator_body["status"], "dataset_unreadable", "{operator}");
    let operator_message = operator_body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("a refusal must say why: {operator}"));
    assert!(
        operator_message.contains("<dataset>/project.json"),
        "the operator must be told which file failed, relative to the dataset: \
         {operator_message}",
    );
    let fixture_path = fixture.dataset_dir().display().to_string();
    assert!(
        !operator_message.contains(&fixture_path),
        "a refusal must not report where the server keeps its files: {operator_message}",
    );
}

/// Find one certificate in a `get_certificates` answer.
fn certificate_named<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["certificates"]
        .as_array()
        .unwrap_or_else(|| panic!("no certificates array: {body}"))
        .iter()
        .find(|certificate| certificate["id"] == id)
        .unwrap_or_else(|| panic!("`{id}` is not in the answer: {body}"))
}

// ---------------------------------------------------------------------------
// whats_changed: the client carries the cursor, the server carries nothing
// ---------------------------------------------------------------------------

/// Every change id a `whats_changed` answer reported.
fn reported(body: &Value) -> Vec<String> {
    body["changes"]
        .as_array()
        .unwrap_or_else(|| panic!("no changes array: {body}"))
        .iter()
        .filter_map(|change| change["id"].as_str().map(str::to_owned))
        .collect()
}

/// The cursor an answer handed back.
fn cursor_of(body: &Value) -> String {
    body["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("an answer must hand back a cursor: {body}"))
        .to_owned()
}

#[tokio::test]
async fn whats_changed_reports_once_per_cursor_and_carries_across_sessions() {
    let fixture = Fixture::new("whats-changed");
    let server = TestServer::start(fixture.config()).await;

    let first_session = session_for(&server, BUYER).await;
    let first = call_tool(
        &server,
        BUYER,
        first_session.as_deref(),
        "whats_changed",
        json!({}),
    )
    .await;
    let first_body = structured(&first, false);
    let changes = first_body["changes"]
        .as_array()
        .unwrap_or_else(|| panic!("no changes array: {first}"));
    assert!(
        !changes.is_empty(),
        "a caller with no cursor must be told what changed since the baseline: {first}",
    );
    for change in changes {
        assert!(
            change["citations"]
                .as_array()
                .is_some_and(|citations| !citations.is_empty()),
            "a change with no citation must not be reported: {change}",
        );
    }
    let cursor = cursor_of(first_body);
    assert!(cursor.starts_with("v1."), "{first}");
    assert_eq!(first_body["delivery"]["carried_forward"], 0, "{first}");

    // A new session, carrying the cursor: the MCP session is gone, the mark is
    // not — because the client is holding it.
    let second_session = session_for(&server, BUYER).await;
    assert_ne!(
        first_session, second_session,
        "this test only means something across two different sessions",
    );
    let second = call_tool(
        &server,
        BUYER,
        second_session.as_deref(),
        "whats_changed",
        json!({"since_cursor": cursor}),
    )
    .await;
    let second_body = structured(&second, false);
    assert!(
        reported(second_body).is_empty(),
        "a change the cursor accounts for must not repeat: {second}",
    );
    assert_eq!(
        second_body["delivery"]["carried_forward"],
        changes.len(),
        "{second}",
    );
    assert_eq!(
        cursor_of(second_body),
        cursor_of(first_body),
        "nothing new was delivered, so the cursor does not move: {second}",
    );

    // A different token is a different caller, and holds no cursor of its own.
    let developer_session = session_for(&server, DEVELOPER).await;
    let developer = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "whats_changed",
        json!({}),
    )
    .await;
    assert!(
        !reported(structured(&developer, false)).is_empty(),
        "what one caller has been told must not answer for another: {developer}",
    );
}

/// R2: a call the client gives up on costs the caller nothing.
///
/// Under the previous build the server recorded delivery as soon as it had
/// composed the answer, so a response lost in transport took both changes with
/// it permanently. Nothing is recorded now: the mark moves only when the
/// client sends back a cursor, and a cursor it never received cannot be sent.
#[tokio::test]
async fn a_change_survives_a_call_the_client_abandoned() {
    let fixture = Fixture::new("abandoned-call");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let expected = reported(structured(
        &call_tool(
            &server,
            BUYER,
            session.as_deref(),
            "whats_changed",
            json!({}),
        )
        .await,
        false,
    ));
    assert_eq!(
        expected.len(),
        2,
        "the committed record dates two changes after the baseline for a buyer",
    );
    for attempt in 1..=5 {
        // A client that stops waiting. Whether the answer was composed, sent,
        // or half-sent is not knowable from here — which is the point.
        server.abandon(BUYER, "whats_changed", &json!({})).await;

        let retry = call_tool(
            &server,
            BUYER,
            session.as_deref(),
            "whats_changed",
            json!({}),
        )
        .await;
        assert_eq!(
            reported(structured(&retry, false)),
            expected,
            "attempt {attempt}: a change whose answer never reached the caller must still be \
             offered: {retry}",
        );
    }

    // And once the client does receive a cursor and sends it back, the changes
    // stop — so this is not simply a tool that repeats itself forever.
    let received = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "whats_changed",
        json!({}),
    )
    .await;
    let received_body = structured(&received, false);
    assert_eq!(
        reported(received_body),
        expected,
        "the call whose cursor is about to be sent back must itself have delivered the \
         changes: {received}",
    );
    let cursor = cursor_of(received_body);
    let acknowledged = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "whats_changed",
        json!({"since_cursor": cursor}),
    )
    .await;
    assert!(
        reported(structured(&acknowledged, false)).is_empty(),
        "a cursor the client actually received must settle the changes it covers: {acknowledged}",
    );
}

/// B4: the older of two changes fails verification on the first call. The
/// newer one is reported and the older one is not — and when its evidence
/// verifies again, the caller is told about it.
#[tokio::test]
async fn whats_changed_re_offers_a_change_whose_evidence_failed_verification() {
    let fixture = Fixture::new("whats-changed-kept");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    // CERT-STRUCT-2026-114 is dated 2026-09-04, CERT-ESCROW-2026-209
    // 2026-10-05: the one that cannot be stated is the older of the two.
    fixture.replace_once(STRUCTURAL_CERTIFICATE_DOC, "54.1 MPa", "54.2 MPa");

    let first = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "whats_changed",
        json!({}),
    )
    .await;
    let first_body = structured(&first, false);
    assert_eq!(
        reported(first_body),
        ["CERT-ESCROW-2026-209"],
        "only the change whose evidence verified may be reported: {first}",
    );
    let cursor = cursor_of(first_body);

    fixture.replace_once(STRUCTURAL_CERTIFICATE_DOC, "54.2 MPa", "54.1 MPa");

    let second = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "whats_changed",
        json!({"since_cursor": cursor}),
    )
    .await;
    let second_body = structured(&second, false);
    assert_eq!(
        reported(second_body),
        ["CERT-STRUCT-2026-114"],
        "a change that was never delivered must be offered again once its evidence verifies: \
         {second}",
    );
    let cursor = cursor_of(second_body);

    let third = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "whats_changed",
        json!({"since_cursor": cursor.clone()}),
    )
    .await;
    assert!(
        reported(structured(&third, false)).is_empty(),
        "a change already delivered must not repeat: {third}",
    );

    // Evidence that breaks *after* delivery does not re-announce the change:
    // the cursor still accounts for it, because it is still a change this
    // caller may hear.
    fixture.replace_once(STRUCTURAL_CERTIFICATE_DOC, "54.1 MPa", "54.2 MPa");
    let fourth = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "whats_changed",
        json!({"since_cursor": cursor}),
    )
    .await;
    let fourth_body = structured(&fourth, false);
    assert!(
        reported(fourth_body).is_empty(),
        "a delivered change must not come back when its evidence flaps: {fourth}",
    );
    assert_eq!(
        fourth_body["delivery"]["carried_forward"], 2,
        "both delivered changes stay in the cursor: {fourth}",
    );
}

/// R4: a cursor shrinks to what this caller may hear, and never grows.
///
/// MS-10 is a public milestone whose only evidence is CERT-MEP-2026-052, a
/// developer-only certificate. Date both after the baseline and the
/// developer has four changes while the buyer has two — and every other
/// route withholds MS-10's date from the buyer, as the asserts at the end
/// of this test show. `whats_changed` did not. It carried forward any
/// cursor id that matched an event from `dated_events`, and `dated_events`
/// returns a milestone whose evidence the caller may not read with empty
/// citations — there, indistinguishable from evidence that failed
/// verification. So the buyer could send the developer's cursor (b3), or
/// sixty-two guessed ids covering MS-10 and CERT-MEP on every day of
/// October (b5), and have `milestone:MS-10:2026-10-12` handed straight
/// back: a date confirmed to a caller who may not hear it.
#[tokio::test]
async fn a_cursor_never_confirms_a_change_the_caller_may_not_hear() {
    const HIDDEN_MILESTONE: &str = "milestone:MS-10:2026-10-12";
    const HIDDEN_CERTIFICATE: &str = "certificate:CERT-MEP-2026-052:2026-10-10";

    let fixture = Fixture::new("cursor-scope");
    // The committed record already evidences MS-10 with CERT-MEP and
    // nothing else; only the two dates are new. Re-signed with keys
    // generated into the fixture, so the server verifies the copy against
    // the copy and never against the repository's.
    fixture.replace_once(
        "project.json",
        "\"planned_date\": \"2027-08-31\",\n      \"actual_date\": null",
        "\"planned_date\": \"2027-08-31\",\n      \"actual_date\": \"2026-10-12\"",
    );
    fixture.replace_once(
        "project.json",
        "\"issued_date\": null,",
        "\"issued_date\": \"2026-10-10\",",
    );
    fixture.re_sign();

    let server = TestServer::start(fixture.config()).await;
    let developer_session = session_for(&server, DEVELOPER).await;
    let buyer_session = session_for(&server, BUYER).await;

    // b1: the developer hears all four, so the fixture really does date a
    // change the buyer may not hear.
    let b1 = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "whats_changed",
        json!({}),
    )
    .await;
    let b1_body = structured(&b1, false);
    assert_eq!(
        reported(b1_body),
        [
            "CERT-STRUCT-2026-114",
            "CERT-ESCROW-2026-209",
            "CERT-MEP-2026-052",
            "MS-10",
        ],
        "the developer hears every dated change: {b1}",
    );
    let developers_cursor = cursor_of(b1_body);
    assert!(
        cursor::decode(&developers_cursor)
            .expect("the server's own cursor decodes")
            .contains(HIDDEN_MILESTONE),
        "this test means nothing unless the developer's cursor carries MS-10: {b1}",
    );

    // b2: the buyer hears two, and the answer names neither MS-10 nor the
    // certificate behind it.
    let b2 = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "whats_changed",
        json!({}),
    )
    .await;
    let b2_body = structured(&b2, false);
    let buyers_changes = reported(b2_body);
    assert_eq!(
        buyers_changes,
        ["CERT-STRUCT-2026-114", "CERT-ESCROW-2026-209"],
        "{b2}",
    );
    let buyers_own_cursor =
        cursor::decode(&cursor_of(b2_body)).expect("the buyer's own cursor decodes");

    // Sixty-two guesses: MS-10 and CERT-MEP on every day of October.
    let mut guessed = BTreeSet::new();
    for day in 1..=31 {
        guessed.insert(format!("milestone:MS-10:2026-10-{day:02}"));
        guessed.insert(format!("certificate:CERT-MEP-2026-052:2026-10-{day:02}"));
    }
    assert_eq!(guessed.len(), 62);
    assert!(guessed.contains(HIDDEN_MILESTONE) && guessed.contains(HIDDEN_CERTIFICATE));

    for (label, sent, expected_changes, expected_carried) in [
        (
            "b3: the buyer sends the developer's cursor",
            developers_cursor,
            Vec::new(),
            2,
        ),
        (
            "b5: the buyer sends sixty-two guessed ids",
            cursor::encode(&guessed),
            buyers_changes.clone(),
            0,
        ),
    ] {
        let reply = call_tool(
            &server,
            BUYER,
            buyer_session.as_deref(),
            "whats_changed",
            json!({"since_cursor": sent}),
        )
        .await;
        let body = structured(&reply, false);
        assert_eq!(reported(body), expected_changes, "{label}: {reply}");
        assert_eq!(
            body["delivery"]["carried_forward"], expected_carried,
            "{label}: only a change this caller may hear can be carried forward: {reply}",
        );

        let handed_back = cursor::decode(&cursor_of(body)).expect("the cursor handed back decodes");
        for hidden in [HIDDEN_MILESTONE, HIDDEN_CERTIFICATE] {
            assert!(
                !handed_back.contains(hidden),
                "{label}: `{hidden}` was confirmed back to a caller who may not hear it: \
                 {reply}",
            );
        }
        assert_eq!(
            handed_back, buyers_own_cursor,
            "{label}: a cursor may shrink to the changes this caller could have been told \
             about anyway, and may never grow: {reply}",
        );
        let whole = rendered(&reply);
        for hidden in ["MS-10", "CERT-MEP", "2026-10-12", "2026-10-10"] {
            assert!(
                !whole.contains(hidden),
                "{label}: the answer names `{hidden}`: {whole}",
            );
        }
    }

    // Every other route withholds the same date from the same caller, which
    // is what made the cursor the one way in.
    let milestones = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let withheld: Vec<&str> = structured(&milestones, false)["withheld"]
        .as_array()
        .unwrap_or_else(|| panic!("no withheld array: {milestones}"))
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert!(withheld.contains(&"MS-10"), "{milestones}");
    assert!(
        !rendered(&milestones).contains("2026-10-12"),
        "a withheld milestone must not carry its date: {milestones}",
    );

    let claimed = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "verify_claim",
        claim("milestone", "MS-10", "actual_date", "2026-10-12"),
    )
    .await;
    let claimed_body = structured(&claimed, false);
    assert_eq!(claimed_body["verdict"], "UNVERIFIED", "{claimed}");
    assert_eq!(claimed_body["reason"], "evidence_unverified", "{claimed}");
}

// ---------------------------------------------------------------------------
// Composed answers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ask_project_answers_with_citations_or_says_it_cannot_verify() {
    let fixture = Fixture::new("ask-project");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let answered = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "ask_project",
        json!({"topic": "milestones"}),
    )
    .await;
    let body = structured(&answered, false);
    assert_eq!(body["answered"], true, "{answered}");
    assert_eq!(body["topic"], "milestones", "{answered}");
    let facts = body["facts"]
        .as_array()
        .unwrap_or_else(|| panic!("no facts array: {answered}"));
    assert!(!facts.is_empty(), "{answered}");
    for fact in facts {
        assert!(
            fact["citations"]
                .as_array()
                .is_some_and(|citations| !citations.is_empty()),
            "a fact with no citation must not be stated: {fact}",
        );
    }
    assert!(
        body["answer"]
            .as_str()
            .is_some_and(|answer| answer.contains("MS-07") && answer.contains("SR-06")),
        "the spoken answer must carry its citations: {answered}",
    );

    // Every topic is answerable from the committed record.
    for topic in ["overview", "certificates", "payments", "escrow"] {
        let reply = call_tool(
            &server,
            BUYER,
            session.as_deref(),
            "ask_project",
            json!({"topic": topic}),
        )
        .await;
        assert_eq!(
            structured(&reply, false)["answered"],
            true,
            "`{topic}` must be answerable from the signed record: {reply}",
        );
    }

    // And a topic whose only evidence failed is refused in words, not guessed.
    fixture.tamper("docs/certificate-escrow-attestation-CERT-ESCROW-2026-209.md");
    let refused = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "ask_project",
        json!({"topic": "escrow"}),
    )
    .await;
    let refused_body = structured(&refused, false);
    assert_eq!(refused_body["answered"], false, "{refused}");
    assert!(
        refused_body["answer"]
            .as_str()
            .is_some_and(|answer| answer.starts_with("I cannot verify that")),
        "a topic this call cannot evidence must be refused in words: {refused}",
    );
    assert!(
        refused_body["facts"].as_array().is_some_and(Vec::is_empty),
        "{refused}",
    );
    assert!(
        !rendered(&refused).contains("48213900000"),
        "the balance must not be stated while its attestation does not verify: {refused}",
    );
}

#[tokio::test]
async fn a_missing_argument_is_a_protocol_error_that_names_it() {
    let fixture = Fixture::new("missing-argument");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let reply = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "get_payment_status",
        json!({}),
    )
    .await;
    let error = protocol_error(&reply);
    assert!(
        error.contains("`unit`") && error.contains("get_payment_status"),
        "the error must name the tool and the argument: {error}",
    );
}

// ---------------------------------------------------------------------------
// Evidence cards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn evidence_cards_are_listed_and_read_as_ui_resources_within_the_role() {
    let fixture = Fixture::new("evidence-cards");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let response = server
        .post(
            BUYER,
            session.as_deref(),
            &json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"}),
        )
        .await;
    let listed = message(response).await;
    let resources = listed["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("resources/list returned no array: {listed}"));
    assert!(!resources.is_empty(), "{listed}");
    for resource in resources {
        let uri = resource["uri"].as_str().unwrap_or_default();
        assert!(
            uri.starts_with("ui://ask-aunova/evidence/"),
            "every evidence card is a ui:// resource: {resource}",
        );
        assert_eq!(
            resource["mimeType"], "text/html;profile=mcp-app",
            "{resource}"
        );
    }
    let uris = rendered(&listed);
    assert!(
        uris.contains("ui://ask-aunova/evidence/PS-1204"),
        "{listed}"
    );
    for hidden in ["CN-14", "CERT-MEP-2026-052", "PS-0802"] {
        assert!(
            !uris.contains(hidden),
            "a buyer must not see `{hidden}` in a resource listing: {uris}",
        );
    }

    let response = server
        .post(
            BUYER,
            session.as_deref(),
            &json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "resources/read",
                "params": {"uri": "ui://ask-aunova/evidence/CERT-FIRE-2026-087"}
            }),
        )
        .await;
    let card = message(response).await;
    let contents = &card["result"]["contents"][0];
    assert_eq!(contents["mimeType"], "text/html;profile=mcp-app", "{card}");
    let html = contents["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the card carries no text: {card}"));
    assert!(html.contains("signature: verified"), "{html}");
    assert!(html.contains("Northwind Fire Safety Institute"), "{html}");
    assert!(html.contains("2026-07-18"), "{html}");
    assert!(html.contains("SYNTHETIC DEMO DATA"), "{html}");
    for remote in ["http://", "https://", "<script"] {
        assert!(
            !html.contains(remote),
            "an evidence card must fetch nothing: found `{remote}`",
        );
    }

    // A card the caller may not read is answered exactly like one that does
    // not exist.
    let response = server
        .post(
            BUYER,
            session.as_deref(),
            &json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "resources/read",
                "params": {"uri": "ui://ask-aunova/evidence/CN-14"}
            }),
        )
        .await;
    let refused = message(response).await;
    assert!(
        refused["result"].is_null() && refused["error"].is_object(),
        "a buyer must not read the internal note's card: {refused}",
    );

    let developer_session = session_for(&server, DEVELOPER).await;
    let response = server
        .post(
            DEVELOPER,
            developer_session.as_deref(),
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "resources/read",
                "params": {"uri": "ui://ask-aunova/evidence/CN-14"}
            }),
        )
        .await;
    let allowed = message(response).await;
    assert!(
        allowed["result"]["contents"][0]["text"]
            .as_str()
            .is_some_and(|html| html.contains("CN-14")),
        "the developer may read the internal note's card: {allowed}",
    );
}

#[tokio::test]
async fn an_evidence_card_shows_the_break_the_moment_it_happens() {
    let fixture = Fixture::new("card-after-tamper");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;
    let request = json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "resources/read",
        "params": {"uri": "ui://ask-aunova/evidence/CERT-STRUCT-2026-114"}
    });

    let before = message(server.post(BUYER, session.as_deref(), &request).await).await;
    assert!(
        before["result"]["contents"][0]["text"]
            .as_str()
            .is_some_and(|html| html.contains("signature: verified")),
        "{before}",
    );

    fixture.tamper(STRUCTURAL_CERTIFICATE_DOC);

    let after = message(server.post(BUYER, session.as_deref(), &request).await).await;
    let html = after["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the card carries no text: {after}"));
    assert!(html.contains("signature: signature_invalid"), "{html}");
    assert!(!html.contains("signature: verified"), "{html}");
}

// ---------------------------------------------------------------------------
// The token layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_without_a_token_is_refused_with_401() {
    let fixture = Fixture::new("no-token");
    let server = TestServer::start(fixture.config()).await;
    let response = server
        .client
        .post(&server.url)
        .header(header::ACCEPT, ACCEPT)
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .send()
        .await
        .expect("the server answered");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer"),
    );

    let body = response.text().await.expect("read the 401 body");
    assert!(
        !body.contains("demo-token"),
        "the 401 body must not reveal which tokens exist: {body}",
    );
}

#[tokio::test]
async fn a_malformed_or_unknown_token_is_refused_with_401() {
    let fixture = Fixture::new("bad-token");
    let server = TestServer::start(fixture.config()).await;

    for authorization in [
        "Basic demo-token-developer",
        "demo-token-developer",
        "Bearer ",
        "Bearer not-a-demo-identity",
    ] {
        let response = server
            .client
            .post(&server.url)
            .header(header::AUTHORIZATION, authorization)
            .header(header::ACCEPT, ACCEPT)
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .send()
            .await
            .expect("the server answered");

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "`{authorization}` must not reach the MCP service",
        );
        assert!(
            response.headers().contains_key(header::WWW_AUTHENTICATE),
            "`{authorization}` was refused without a WWW-Authenticate challenge",
        );
    }
}

// ---------------------------------------------------------------------------
// R3: a unit outside the token is refused, never substituted
// ---------------------------------------------------------------------------

/// Every unit reference the previous review's 41 scope questions put in front
/// of a buyer, as the one place the new schema can carry one.
///
/// The questions themselves are no longer expressible — there is no sentence
/// to parse — so what is left of them is the argument: whatever a calling
/// model resolved "unit 802", "the flat on floor 8" or "٠٨٠٢" to, it arrives
/// here, and anything that is not this buyer's own unit is refused.
const UNITS_OUTSIDE_THE_BUYERS_SCOPE: [&str; 10] = [
    "0802",
    "802",
    "08 02",
    "٠٨٠٢",
    "０８０２",
    "1510",
    "15-10",
    "0901",
    "floor 8",
    "the other buyers",
];

/// Figures and ids that would prove a refusal had been answered about unit
/// 1204 instead of refused.
const SUBSTITUTED_1204: [&str; 5] = ["147000000", "1470000.00", "PS-1204", "IN-06", "245000000"];

#[tokio::test]
async fn ask_project_refuses_a_unit_outside_the_token_and_never_substitutes_one() {
    let fixture = Fixture::new("ask-scope");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    for unit in UNITS_OUTSIDE_THE_BUYERS_SCOPE {
        for topic in ["payments", "milestones", "overview"] {
            let reply = call_tool(
                &server,
                BUYER,
                session.as_deref(),
                "ask_project",
                json!({"topic": topic, "unit": unit}),
            )
            .await;
            let body = structured(&reply, true);
            assert_eq!(body["status"], "refused", "`{unit}`/{topic}: {reply}");
            assert_eq!(body["reason"], "role_scope", "`{unit}`/{topic}: {reply}");
            assert_eq!(body["answered"], false, "`{unit}`/{topic}: {reply}");

            let leaked = rendered(&reply);
            for substituted in SUBSTITUTED_1204 {
                assert!(
                    !leaked.contains(substituted),
                    "a refused call must not be answered about the caller's own unit \
                     (`{substituted}`): {leaked}",
                );
            }
            assert!(
                !leaked.contains(unit),
                "a refusal must not echo the caller's own argument back: {leaked}",
            );
        }
    }

    // Omitting the unit is the caller saying "mine", which is honest and is
    // the question a buyer actually asks.
    let own = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "ask_project",
        json!({"topic": "payments"}),
    )
    .await;
    let own_body = structured(&own, false);
    assert_eq!(own_body["answered"], true, "{own}");
    assert_eq!(
        own_body["units"].as_array().map(Vec::len),
        Some(1),
        "a buyer's own answer is about their own unit and no other: {own}",
    );
    assert_eq!(own_body["units"][0], "1204", "{own}");
    assert!(
        rendered(&own).contains("PS-1204"),
        "the buyer's own schedule must still answer their own question: {own}",
    );

    // Naming their own unit explicitly is the same answer, not a refusal.
    let named = call_tool(
        &server,
        BUYER,
        session.as_deref(),
        "ask_project",
        json!({"topic": "payments", "unit": "1204"}),
    )
    .await;
    assert_eq!(structured(&named, false)["answered"], true, "{named}");

    // The developer may read every unit, and an unknown one is not found
    // rather than refused.
    let developer_session = session_for(&server, DEVELOPER).await;
    let every = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "ask_project",
        json!({"topic": "payments"}),
    )
    .await;
    assert_eq!(
        structured(&every, false)["units"].as_array().map(Vec::len),
        Some(3),
        "the developer hears about every unit: {every}",
    );
    let unknown = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "ask_project",
        json!({"topic": "payments", "unit": "0901"}),
    )
    .await;
    assert_eq!(
        structured(&unknown, true)["status"],
        "not_found",
        "{unknown}",
    );
}

#[tokio::test]
async fn verify_claim_refuses_a_unit_outside_the_token_and_never_substitutes_one() {
    let fixture = Fixture::new("claim-scope-units");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    for unit in UNITS_OUTSIDE_THE_BUYERS_SCOPE {
        for (field, value) in [
            ("paid_to_date_minor", "89100000"),
            ("has_overdue_instalment", "true"),
            ("total_price_minor", "198000000"),
        ] {
            let reply = call_tool(
                &server,
                BUYER,
                session.as_deref(),
                "verify_claim",
                claim("unit", unit, field, value),
            )
            .await;
            let body = structured(&reply, false);
            assert_eq!(body["verdict"], "UNVERIFIED", "`{unit}`/{field}: {reply}");
            assert_eq!(body["reason"], "role_scope", "`{unit}`/{field}: {reply}");
            assert!(
                body["subject"].is_null(),
                "a refused claim resolves no subject: {reply}",
            );

            let leaked = rendered(&reply);
            for secret in ["PS-0802", "89100000", "PS-1510", "187200000", "29700000"] {
                assert!(
                    !leaked.contains(secret),
                    "a refused claim must not leak another unit (`{secret}`): {leaked}",
                );
            }
            for substituted in SUBSTITUTED_1204 {
                assert!(
                    !leaked.contains(substituted),
                    "a refused claim must not be judged against the caller's own unit \
                     (`{substituted}`): {leaked}",
                );
            }
        }
    }

    // The developer may read the unit the buyer was refused, and the record
    // answers for it.
    let developer_session = session_for(&server, DEVELOPER).await;
    let judged = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "verify_claim",
        claim("unit", "0802", "has_overdue_instalment", "true"),
    )
    .await;
    let judged_body = structured(&judged, false);
    assert_eq!(judged_body["verdict"], "SUPPORTED", "{judged}");
    assert_eq!(judged_body["subject"]["id"], "0802", "{judged}");
}

// ---------------------------------------------------------------------------
// R1: a true claim this server did not read is never called false
// ---------------------------------------------------------------------------

/// One status field of one subject, and the spellings a caller sends it in.
#[derive(Clone, Copy)]
struct StatusCase {
    subject_type: &'static str,
    subject_id: &'static str,
    instalment_id: Option<&'static str>,
    field: &'static str,
    /// The record's own word for it.
    truth: &'static str,
    /// Another word of the same closed vocabulary. The record does not use
    /// it, so it is CONTRADICTED — which is what shows the field is judged
    /// at all and the UNVERIFIED answers below are not vacuous.
    other: &'static str,
    /// Spellings of `truth` this server does not read: the same true claim,
    /// written the ways a calling model writes it.
    respelt: &'static [&'static str],
}

impl StatusCase {
    fn arguments(self, value: &str) -> Value {
        let mut arguments = claim(self.subject_type, self.subject_id, self.field, value);
        if let Some(instalment) = self.instalment_id {
            arguments["instalment_id"] = json!(instalment);
        }
        arguments
    }
}

/// R1, status class: the record's own word in another spelling is
/// UNVERIFIED, and never CONTRADICTED.
///
/// The r3 review sent 132 of these and got CONTRADICTED for every one. The
/// record says `in_progress`; a calling model writes `In_progress`,
/// `IN_PROGRESS`, `in progress` or `in-progress`, or inflects `complete`
/// into `completed`. Each of those is a **true** claim about the milestone
/// written in a form this server does not read, and answering CONTRADICTED
/// told the caller the record says otherwise. It does not. A status is one
/// word of that field's closed vocabulary; anything else is read as nothing
/// and compared with nothing.
///
/// Both tokens, because the r3 table ran developer-only and so never saw
/// its own buyer-token probes.
#[tokio::test]
async fn a_true_status_in_another_spelling_is_unverified_and_never_contradicted() {
    let fixture = Fixture::new("status-spelling");
    let server = TestServer::start(fixture.config()).await;

    let cases = [
        StatusCase {
            subject_type: "milestone",
            subject_id: "MS-07",
            instalment_id: None,
            field: "status",
            truth: "in_progress",
            other: "complete",
            respelt: &["In_progress", "IN_PROGRESS", "in progress", "in-progress"],
        },
        StatusCase {
            subject_type: "milestone",
            subject_id: "MS-01",
            instalment_id: None,
            field: "status",
            truth: "complete",
            other: "not_started",
            respelt: &["completed", "Complete", "COMPLETE"],
        },
        StatusCase {
            subject_type: "unit",
            subject_id: "1204",
            instalment_id: Some("IN-04"),
            field: "instalment_status",
            truth: "paid",
            other: "overdue",
            respelt: &["Paid", "PAID", "paid-late"],
        },
        StatusCase {
            subject_type: "certificate",
            subject_id: "CERT-STRUCT-2026-114",
            instalment_id: None,
            field: "status",
            truth: "issued",
            other: "pending",
            respelt: &["Issued", "ISSUED", "issue"],
        },
    ];

    for token in [DEVELOPER, BUYER] {
        let session = session_for(&server, token).await;
        for case in cases {
            let (subject_id, truth) = (case.subject_id, case.truth);

            // The field is judged at all: its own word is SUPPORTED and
            // another word of the same vocabulary is CONTRADICTED. Without
            // these two, UNVERIFIED everywhere would pass vacuously.
            let supported = call_tool(
                &server,
                token,
                session.as_deref(),
                "verify_claim",
                case.arguments(truth),
            )
            .await;
            assert_eq!(
                structured(&supported, false)["verdict"],
                "SUPPORTED",
                "{token}: `{truth}` is what the record says about {subject_id}: {supported}",
            );
            let contradicted = call_tool(
                &server,
                token,
                session.as_deref(),
                "verify_claim",
                case.arguments(case.other),
            )
            .await;
            assert_eq!(
                structured(&contradicted, false)["verdict"],
                "CONTRADICTED",
                "{token}: `{}` is a word of the vocabulary the record does not use for \
                 {subject_id}: {contradicted}",
                case.other,
            );

            for value in case.respelt.iter().copied() {
                let reply = call_tool(
                    &server,
                    token,
                    session.as_deref(),
                    "verify_claim",
                    case.arguments(value),
                )
                .await;
                let body = structured(&reply, false);
                assert_eq!(
                    body["verdict"], "UNVERIFIED",
                    "{token}: `{value}` is a true claim about {subject_id} this server did not \
                     read, so it may not be called false: {reply}",
                );
                assert_eq!(
                    body["reason"], "assertion_not_normalised",
                    "{token}: `{value}`: {reply}",
                );
                let finding = body["finding"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a refusal must say why: {reply}"));
                assert!(
                    finding.contains(truth),
                    "{token}: the refusal must tell the caller the word it does read \
                     (`{truth}`): {finding}",
                );
            }
        }
    }
}

/// R1, money class: the money rule every client is sent is true of what this
/// server does with the value.
///
/// The schema promised that `1,470,000.00 AED`, `1.47m` **and `1470000`**
/// were "all refused rather than guessed at", while the server read any run
/// of digits as minor units. So a buyer who had paid 1,470,000 AED and
/// stated it in dirhams — trusting the stated refusal to catch it — was told
/// CONTRADICTED. A bare integer carries nothing that tells dirhams from
/// fils, so the rule is what changed: every integer is minor units. This
/// pins the promise and the behaviour to each other in one test, in the
/// schema a calling model actually reads.
#[tokio::test]
async fn the_money_rule_in_the_schema_is_what_the_server_does() {
    let fixture = Fixture::new("money-rule");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, BUYER).await;

    let response = server
        .post(
            BUYER,
            session.as_deref(),
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
        )
        .await;
    let listed = message(response).await;
    let description = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no tools array: {listed}"))
        .iter()
        .find(|tool| tool["name"] == "verify_claim")
        .map(|tool| &tool["inputSchema"]["properties"]["asserted_value"]["description"])
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("`verify_claim` describes no `asserted_value`: {listed}"))
        .to_owned();

    for stated in [
        "Every integer is read as minor units",
        "`1470000` states 14,700.00 AED",
        "1,470,000.00 AED is written `147000000`",
    ] {
        assert!(
            description.contains(stated),
            "the money rule a client is sent must say `{stated}`: {description}",
        );
    }

    // What the rule says happens, happening. Unit 1204 has paid
    // 147,000,000 fils, which is 1,470,000.00 AED. An empty reason means
    // the value was read and compared; `assertion_not_normalised` means it
    // was read as nothing.
    let cases: [(&str, &str, &str); 12] = [
        ("147000000", "SUPPORTED", ""),
        // The r3 review's 40: the same sum in whole dirhams. It is a
        // different number of fils, the rule says so in the caller's own
        // schema, and it is compared as such.
        ("1470000", "CONTRADICTED", ""),
        ("1,470,000.00 AED", "UNVERIFIED", "assertion_not_normalised"),
        ("147,000,000", "UNVERIFIED", "assertion_not_normalised"),
        ("1 470 000", "UNVERIFIED", "assertion_not_normalised"),
        ("1470000.00", "UNVERIFIED", "assertion_not_normalised"),
        ("1.47m", "UNVERIFIED", "assertion_not_normalised"),
        ("147M", "UNVERIFIED", "assertion_not_normalised"),
        ("147000000 AED", "UNVERIFIED", "assertion_not_normalised"),
        ("AED 147000000", "UNVERIFIED", "assertion_not_normalised"),
        ("$147000000", "UNVERIFIED", "assertion_not_normalised"),
        ("د.إ 147000000", "UNVERIFIED", "assertion_not_normalised"),
    ];

    for (value, expected, reason) in cases {
        let reply = call_tool(
            &server,
            BUYER,
            session.as_deref(),
            "verify_claim",
            claim("unit", "1204", "paid_to_date_minor", value),
        )
        .await;
        let body = structured(&reply, false);
        assert_eq!(
            body["verdict"], expected,
            "`{value}`: the schema's money rule says this is {expected}: {reply}",
        );
        if reason.is_empty() {
            continue;
        }
        assert_eq!(body["reason"], reason, "`{value}`: {reply}");
        let finding = body["finding"]
            .as_str()
            .unwrap_or_else(|| panic!("a refusal must say why: {reply}"));
        assert!(
            finding.contains("Every integer is read as minor units"),
            "the refusal must quote back the same rule the schema sent: {finding}",
        );
    }
}

// ---------------------------------------------------------------------------
// The field table: every field of every subject, true and false
// ---------------------------------------------------------------------------

/// The vocabulary a milestone's `status` is written from.
const MILESTONE_STATUSES: &[&str] = &["complete", "in_progress", "not_started"];
/// The vocabulary a certificate's `status` is written from.
const CERTIFICATE_STATUSES: &[&str] = &["issued", "pending"];
/// The vocabulary an instalment's `instalment_status` is written from.
const INSTALMENT_STATUSES: &[&str] = &["scheduled", "paid", "paid_late", "overdue"];

/// The reasons a role may be given *instead of* the one the record alone
/// implies, without that being a different answer.
///
/// Each is decided before the stated value is read: the subject is not this
/// caller's (`role_scope`), it is answered exactly as one that does not
/// exist (`subject_not_resolved`), or no document this caller may read
/// evidences it (`evidence_unverified`). The verdict is UNVERIFIED either
/// way, so a role withholding earlier can never turn a true claim false.
const WITHHELD_REASONS: [&str; 3] = ["role_scope", "subject_not_resolved", "evidence_unverified"];

/// How a stated value for a field is read, mirrored here from the subject
/// type and the field name alone so the table and the server reach it by two
/// routes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Money,
    Date,
    Percent,
    Boolean,
    /// A closed vocabulary, written here rather than read from the server, so
    /// a vocabulary that quietly changed would show up as a failure and not
    /// as agreement.
    Vocabulary(&'static [&'static str]),
    Text,
}

impl Kind {
    fn of(subject_type: &str, field: &str) -> Self {
        match (subject_type, field) {
            (_, "total_price_minor" | "paid_to_date_minor" | "outstanding_minor")
            | (_, "instalment_amount_minor") => Kind::Money,
            (
                _,
                "planned_date"
                | "actual_date"
                | "issued_date"
                | "expires_date"
                | "instalment_due_date"
                | "instalment_paid_date",
            ) => Kind::Date,
            (_, "percent_complete" | "percent_of_works" | "instalment_percent") => Kind::Percent,
            (_, "has_overdue_instalment") => Kind::Boolean,
            ("milestone", "status") => Kind::Vocabulary(MILESTONE_STATUSES),
            ("certificate", "status") => Kind::Vocabulary(CERTIFICATE_STATUSES),
            (_, "instalment_status") => Kind::Vocabulary(INSTALMENT_STATUSES),
            _ => Kind::Text,
        }
    }

    /// Values of the right shape that the record cannot be stating, because
    /// they differ from the truth. Each one must come back CONTRADICTED.
    fn wrong(self, truth: &str) -> Vec<String> {
        let mut values = match self {
            Kind::Money => {
                let amount: i64 = truth.parse().expect("a money field is an integer");
                vec![
                    (amount + 1).to_string(),
                    if amount == 0 { "1" } else { "0" }.to_owned(),
                    // The r3 money class: the same sum written in whole
                    // dirhams. A bare integer has no property that tells it
                    // apart from one written in fils, so it is read as fils,
                    // compared, and found different — which is what the
                    // schema now says will happen.
                    (amount / 100).to_string(),
                ]
            }
            Kind::Date => vec!["1999-01-01".to_owned(), "2099-12-31".to_owned()],
            Kind::Percent => {
                let percent: u32 = truth.parse().expect("a percent field is an integer");
                vec![
                    ((percent + 1) % 101).to_string(),
                    ((percent + 7) % 101).to_string(),
                ]
            }
            // A flag has exactly one other value it could have. A second
            // "wrong" value would have to be one that does not parse, and
            // those are covered by `malformed` below.
            Kind::Boolean => vec![(truth != "true").to_string()],
            // Every other word of the same vocabulary: well formed, and not
            // what the record says.
            Kind::Vocabulary(words) => words
                .iter()
                .filter(|word| **word != truth)
                .map(|word| (*word).to_owned())
                .collect(),
            Kind::Text => vec![
                format!("{truth} (amended)"),
                "ZZ-NOT-IN-THE-RECORD".to_owned(),
            ],
        };
        values.retain(|value| value != truth);
        values.sort();
        values.dedup();
        values
    }

    /// Values that are not in the field's canonical form. Each one must come
    /// back UNVERIFIED `assertion_not_normalised`, never compared loosely —
    /// and that holds whether the value is the record's own figure in another
    /// notation or a different one, because this server did not read either.
    fn malformed(self, truth: &str) -> Vec<String> {
        let fixed: &[&str] = match self {
            // The r3 money class, in full: a thousands separator (comma or
            // space), a currency code before or after, a currency symbol
            // (ASCII and the dirham's own, which is not), a decimal point,
            // and a scale suffix. Not one of these is read, so not one of
            // them may be compared — the bare integer beside them is the
            // only canonical form, and it is covered by `wrong` above.
            Kind::Money => &[
                "1,470,000.00 AED",
                "147,000,000",
                "1 470 000",
                "1470000.00",
                "147000000.0",
                "1.47m",
                "147M",
                "147000000 AED",
                "AED 147000000",
                "147000000 USD",
                "$147000000",
                "د.إ 147000000",
            ],
            Kind::Date => &["2026-9-4", "04/09/2026"],
            Kind::Percent => &["48%", "101"],
            Kind::Boolean => &["yes", "True"],
            // The r3 status class: the record's own word, respelled. Every
            // one of these is a *true* claim this server did not read, so
            // UNVERIFIED is the only honest answer — calling it CONTRADICTED
            // would be calling a true claim false.
            Kind::Vocabulary(_) => &[],
            // Any non-empty string is a readable text value; an empty one is
            // refused by the schema, which the schema-error test covers.
            Kind::Text => &[],
        };
        let mut values: Vec<String> = fixed.iter().map(|value| (*value).to_owned()).collect();
        if matches!(self, Kind::Vocabulary(_)) {
            values.extend(respellings(truth));
        }
        values.retain(|value| value != truth);
        values.sort();
        values.dedup();
        values
    }

    /// A well-formed value, for probing a field the record does not state.
    fn well_formed(self) -> &'static str {
        match self {
            Kind::Money => "1",
            Kind::Date => "2026-01-01",
            Kind::Percent => "50",
            Kind::Boolean => "true",
            Kind::Vocabulary(words) => words[0],
            Kind::Text => "anything",
        }
    }
}

/// The record's own word, written the ways a caller's model is likely to
/// write it: capitalised, upper case, with the underscore as a space or a
/// hyphen, and inflected. All four classes of the r3 review's 132.
///
/// No form here differs from the record's word by whitespace alone: trimming
/// is the one step applied before any rule, so ` complete ` *is* `complete`,
/// and SUPPORTED is the right answer for it.
fn respellings(word: &str) -> Vec<String> {
    let mut forms = vec![
        word.to_uppercase(),
        word.replace('_', " "),
        word.replace('_', "-"),
        format!("{word}d"),
    ];
    let mut capitalised = word.to_owned();
    if let Some(first) = capitalised.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    forms.push(capitalised);
    forms
}

/// One row of the field table: one field of one subject.
struct FieldCase {
    subject_type: &'static str,
    subject_id: String,
    instalment_id: Option<String>,
    field: &'static str,
    /// The record's own value, read from `project.json` as raw JSON. `None`
    /// means the record states none.
    truth: Option<String>,
    /// Whether the record names any document that evidences this subject at
    /// all. A subject evidenced by nothing is never judged.
    evidenced: bool,
}

impl FieldCase {
    fn arguments(&self, value: &str) -> Value {
        let mut arguments = claim(self.subject_type, &self.subject_id, self.field, value);
        if let Some(instalment) = &self.instalment_id {
            arguments["instalment_id"] = json!(instalment);
        }
        arguments
    }

    fn label(&self) -> String {
        match &self.instalment_id {
            Some(instalment) => format!(
                "{} {} {} {}",
                self.subject_type, self.subject_id, instalment, self.field
            ),
            None => format!("{} {} {}", self.subject_type, self.subject_id, self.field),
        }
    }
}

/// A raw JSON value as the string a caller would state for it.
fn stated(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        Some(Value::Bool(flag)) => Some(flag.to_string()),
        Some(other) => panic!("a checkable field is a scalar, got {other}"),
    }
}

/// Build the whole table from the committed record.
fn field_table() -> Vec<FieldCase> {
    let record = committed_record();
    let mut cases = Vec::new();

    let issuer_names: BTreeMap<String, String> = record["issuers"]
        .as_array()
        .expect("the record lists issuers")
        .iter()
        .map(|issuer| {
            (
                issuer["id"]
                    .as_str()
                    .expect("an issuer has an id")
                    .to_owned(),
                issuer["name"]
                    .as_str()
                    .expect("an issuer has a name")
                    .to_owned(),
            )
        })
        .collect();

    for milestone in record["milestones"]
        .as_array()
        .expect("the record lists milestones")
    {
        let id = milestone["id"].as_str().expect("a milestone has an id");
        for field in [
            "status",
            "title",
            "planned_date",
            "actual_date",
            "percent_complete",
            "percent_of_works",
        ] {
            cases.push(FieldCase {
                subject_type: "milestone",
                subject_id: id.to_owned(),
                instalment_id: None,
                field,
                truth: stated(milestone.get(field)),
                // MS-09 is evidenced by nothing at all, which is a statement
                // about the record and not about this call's verification.
                evidenced: milestone["evidence"]
                    .as_array()
                    .is_some_and(|evidence| !evidence.is_empty())
                    || milestone.get("contractor_note").is_some(),
            });
        }
    }

    for certificate in record["certificates"]
        .as_array()
        .expect("the record lists certificates")
    {
        let id = certificate["id"].as_str().expect("a certificate has an id");
        for field in [
            "status",
            "title",
            "scope",
            "issued_date",
            "expires_date",
            "issuer_id",
        ] {
            let source = if field == "issuer_id" {
                "issuer"
            } else {
                field
            };
            cases.push(FieldCase {
                subject_type: "certificate",
                subject_id: id.to_owned(),
                instalment_id: None,
                field,
                truth: stated(certificate.get(source)),
                // Every certificate names the document that is its evidence.
                evidenced: true,
            });
        }
        // `issuer` is the display name the record gives that issuer id.
        cases.push(FieldCase {
            subject_type: "certificate",
            subject_id: id.to_owned(),
            instalment_id: None,
            field: "issuer",
            truth: certificate["issuer"]
                .as_str()
                .and_then(|id| issuer_names.get(id))
                .cloned(),
            evidenced: true,
        });
    }

    for schedule in record["payment_schedules"]
        .as_array()
        .expect("the record lists payment schedules")
    {
        let unit = schedule["unit"].as_str().expect("a schedule has a unit");
        let instalments = schedule["instalments"]
            .as_array()
            .expect("a schedule has instalments");
        let total = schedule["total_price_minor"]
            .as_i64()
            .expect("a schedule has a price");
        let paid = schedule["paid_to_date_minor"]
            .as_i64()
            .expect("a schedule has a paid figure");
        let overdue = instalments
            .iter()
            .any(|instalment| instalment["status"] == "overdue");

        for (field, truth) in [
            ("total_price_minor", Some(total.to_string())),
            ("paid_to_date_minor", Some(paid.to_string())),
            ("outstanding_minor", Some((total - paid).to_string())),
            ("has_overdue_instalment", Some(overdue.to_string())),
            (
                "escrow_account_ref",
                stated(schedule.get("escrow_account_ref")),
            ),
            ("unit_type", stated(schedule.get("type"))),
        ] {
            cases.push(FieldCase {
                subject_type: "unit",
                subject_id: unit.to_owned(),
                instalment_id: None,
                field,
                truth,
                // Every schedule names the signed document that is its
                // evidence.
                evidenced: true,
            });
        }

        for instalment in instalments {
            let instalment_id = instalment["id"].as_str().expect("an instalment has an id");
            for (field, source) in [
                ("instalment_status", "status"),
                ("instalment_amount_minor", "amount_minor"),
                ("instalment_percent", "percent"),
                ("instalment_due_date", "due_date"),
                ("instalment_paid_date", "paid_date"),
                ("instalment_label", "label"),
                ("instalment_trigger", "trigger"),
            ] {
                cases.push(FieldCase {
                    subject_type: "unit",
                    subject_id: unit.to_owned(),
                    instalment_id: Some(instalment_id.to_owned()),
                    field,
                    truth: stated(instalment.get(source)),
                    evidenced: true,
                });
            }
        }
    }

    cases
}

/// What one token's run of the field table found.
struct TableRun {
    token: &'static str,
    probes: usize,
    verdicts: BTreeMap<&'static str, usize>,
    defects: Vec<String>,
}

/// Every field of every subject the record holds: its true value, at least one
/// wrong value of the same shape, and values that are not in the field's
/// canonical form.
///
/// The target is absolute, and it is the one the previous build missed by 101
/// and this one by 172: **no false SUPPORTED, and no true CONTRADICTED.**
///
/// Run with **both** tokens. The developer sees the whole record, so their
/// run is the exact expectation. The buyer sees one unit, the public
/// milestones and the issued certificates, so their run is checked against a
/// rule instead: a role may *withhold* a verdict but may never flip one. The
/// r3 review found the table running developer-only, which is why every
/// buyer-token probe in its 172 was invisible here.
#[tokio::test]
async fn the_record_supports_only_its_own_values_field_by_field() {
    let fixture = Fixture::new("field-table");
    let server = TestServer::start(fixture.config()).await;

    let table = field_table();
    assert!(
        table.len() >= 250,
        "the field table has shrunk to {} rows; it must cover every field of all 3 units \
         including their instalments, all 5 certificates and all 10 milestones",
        table.len(),
    );

    let mut runs = Vec::new();
    for token in [DEVELOPER, BUYER] {
        let session = session_for(&server, token).await;
        let mut run = TableRun {
            token,
            probes: 0,
            verdicts: BTreeMap::new(),
            defects: Vec::new(),
        };

        for case in &table {
            for (value, verdict, reason) in expectations(case) {
                run.probes += 1;
                let reply = call_tool(
                    &server,
                    token,
                    session.as_deref(),
                    "verify_claim",
                    case.arguments(&value),
                )
                .await;
                let body = structured(&reply, false);
                let got = body["verdict"].as_str().unwrap_or("<none>");
                let got_reason = body["reason"].as_str();
                *run.verdicts.entry(verdict_word(got)).or_insert(0) += 1;

                // The developer is told exactly what the record says. The
                // buyer may be told less — never something else: a true
                // value must never come back CONTRADICTED and a false one
                // must never come back SUPPORTED, whoever is asking.
                let acceptable: Vec<&str> = if token == DEVELOPER {
                    vec![verdict]
                } else if verdict == "UNVERIFIED" {
                    vec!["UNVERIFIED"]
                } else {
                    vec![verdict, "UNVERIFIED"]
                };
                // "Less" includes the *reason*. WITHHELD_REASONS are all
                // decided before the stated value is read at all, because
                // this caller cannot reach the subject: unit 0802 is not
                // theirs, CERT-MEP is answered exactly like a certificate
                // that does not exist, and no document they may read
                // evidences MS-10. Each of those displaces the reason the
                // record alone would give, and the verdict is UNVERIFIED
                // either way, so nothing is flipped — only withheld
                // earlier. The developer's reason must still match exactly.
                let reason_held_back = token != DEVELOPER
                    && got == "UNVERIFIED"
                    && got_reason.is_some_and(|reason| WITHHELD_REASONS.contains(&reason));
                let reason_wrong = verdict == "UNVERIFIED"
                    && got == "UNVERIFIED"
                    && reason.is_some()
                    && got_reason != reason
                    && !reason_held_back;

                if !acceptable.contains(&got) || reason_wrong {
                    run.defects.push(format!(
                        "{} = `{value}`: expected {}{}, got {got}{} — {}",
                        case.label(),
                        acceptable.join(" or "),
                        reason.map(|r| format!(" ({r})")).unwrap_or_default(),
                        got_reason.map(|r| format!(" ({r})")).unwrap_or_default(),
                        body["finding"].as_str().unwrap_or("<no finding>"),
                    ));
                }

                if got == "SUPPORTED" || got == "CONTRADICTED" {
                    let evidenced = body["subject"]["citations"]
                        .as_array()
                        .is_some_and(|citations| !citations.is_empty());
                    if !evidenced {
                        run.defects.push(format!(
                            "{} = `{value}`: answered {got} without naming the evidence behind \
                             it — {}",
                            case.label(),
                            body["subject"],
                        ));
                    }
                }
            }
        }
        runs.push(run);
    }

    for run in &runs {
        assert!(
            run.defects.is_empty(),
            "{} defects with the {} token over {} fields and {} probes:\n  {}",
            run.defects.len(),
            run.token,
            table.len(),
            run.probes,
            run.defects.join("\n  "),
        );
        assert!(
            run.probes >= 1_400,
            "only {} probes ran with the {} token over {} fields; the table must not quietly \
             shrink",
            run.probes,
            run.token,
            table.len(),
        );
    }

    // The buyer's run must not be vacuous: their own unit is theirs to read,
    // so every probe on unit 1204 is judged exactly as the developer's is.
    let buyer = runs
        .iter()
        .find(|run| run.token == BUYER)
        .unwrap_or_else(|| panic!("the buyer run is recorded"));
    assert!(
        buyer.verdicts.get("SUPPORTED").copied().unwrap_or(0) > 50
            && buyer.verdicts.get("CONTRADICTED").copied().unwrap_or(0) > 50,
        "a buyer that reached no verdicts proves nothing: {:?}",
        buyer.verdicts,
    );

    for run in &runs {
        println!(
            "field table: {} fields, {} probes with the {} token, verdicts {:?}, 0 false \
             SUPPORTED, 0 true CONTRADICTED",
            table.len(),
            run.probes,
            run.token,
            run.verdicts,
        );
    }
}

/// One verdict word, as a key a count can be kept under.
fn verdict_word(verdict: &str) -> &'static str {
    match verdict {
        "SUPPORTED" => "SUPPORTED",
        "CONTRADICTED" => "CONTRADICTED",
        "UNVERIFIED" => "UNVERIFIED",
        _ => "<none>",
    }
}

/// What the developer's token must be answered for one row of the table.
fn expectations(case: &FieldCase) -> Vec<(String, &'static str, Option<&'static str>)> {
    let kind = Kind::of(case.subject_type, case.field);
    let mut expected = Vec::new();
    match (&case.truth, case.evidenced) {
        // A subject no document evidences is never judged at all, whatever
        // the record says about it — the same rule that withholds MS-09
        // from `get_milestones`.
        (truth, false) => {
            let value = truth
                .clone()
                .unwrap_or_else(|| kind.well_formed().to_owned());
            for value in [value.clone()].into_iter().chain(kind.wrong(&value)) {
                expected.push((value, "UNVERIFIED", Some("evidence_unverified")));
            }
        }
        (Some(truth), true) => {
            expected.push((truth.clone(), "SUPPORTED", None));
            for value in kind.wrong(truth) {
                expected.push((value, "CONTRADICTED", None));
            }
            for value in kind.malformed(truth) {
                expected.push((value, "UNVERIFIED", Some("assertion_not_normalised")));
            }
        }
        (None, true) => {
            // A field the record does not state is never a verdict either
            // way, however well-formed the value stated for it.
            for value in [kind.well_formed().to_owned()]
                .into_iter()
                .chain(kind.wrong(kind.well_formed()))
            {
                expected.push((value, "UNVERIFIED", Some("not_stated_by_record")));
            }
        }
    }
    expected
}

/// The ways an assertion can fail to name something checkable, each of which
/// is UNVERIFIED with its own reason and never a verdict.
#[tokio::test]
async fn an_assertion_that_names_nothing_checkable_is_unverified() {
    let fixture = Fixture::new("unverifiable-claims");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, DEVELOPER).await;

    let cases: [(Value, &str, &str); 6] = [
        (
            claim("milestone", "MS-99", "status", "complete"),
            "subject_not_resolved",
            "a milestone the record does not hold",
        ),
        (
            claim("certificate", "CERT-NOPE", "status", "issued"),
            "subject_not_resolved",
            "a certificate the record does not hold",
        ),
        (
            claim("unit", "9999", "paid_to_date_minor", "1"),
            "subject_not_resolved",
            "a unit the record does not hold",
        ),
        (
            claim("milestone", "MS-07", "issued_date", "2026-09-04"),
            "assertion_not_understood",
            "a certificate's field asked of a milestone",
        ),
        (
            claim("unit", "1204", "instalment_status", "paid"),
            "subject_not_resolved",
            "an instalment field with no instalment named",
        ),
        (
            json!({
                "subject_type": "unit", "subject_id": "1204",
                "field": "paid_to_date_minor", "asserted_value": "147000000",
                "instalment_id": "IN-06"
            }),
            "assertion_not_understood",
            "an instalment named for a field that is not about one",
        ),
    ];

    for (arguments, reason, why) in cases {
        let reply = call_tool(
            &server,
            DEVELOPER,
            session.as_deref(),
            "verify_claim",
            arguments.clone(),
        )
        .await;
        let body = structured(&reply, false);
        assert_eq!(body["verdict"], "UNVERIFIED", "{why}: {reply}");
        assert_eq!(body["reason"], reason, "{why}: {reply}");
    }

    // An instalment id that is not in that schedule resolves nothing either.
    let reply = call_tool(
        &server,
        DEVELOPER,
        session.as_deref(),
        "verify_claim",
        json!({
            "subject_type": "unit", "subject_id": "1510",
            "field": "instalment_status", "asserted_value": "paid",
            "instalment_id": "IN-08"
        }),
    )
    .await;
    let body = structured(&reply, false);
    assert_eq!(body["verdict"], "UNVERIFIED", "{reply}");
    assert_eq!(body["reason"], "subject_not_resolved", "{reply}");
}

// ---------------------------------------------------------------------------
// A dataset `verify-dataset` rejects is not answered from
// ---------------------------------------------------------------------------

/// A manifest that signs one `doc_id` twice is a dataset `verify-dataset`
/// exits non-zero on. Every hash and signature in it still checks out, so this
/// is exactly the fault a per-document status cannot carry — and the server
/// must refuse rather than answer with `documents_verified 14`.
#[tokio::test]
async fn a_dataset_the_verifier_rejects_is_refused_by_every_tool() {
    let fixture = Fixture::new("duplicate-entry");
    let server = TestServer::start(fixture.config()).await;
    let session = session_for(&server, DEVELOPER).await;

    let before = call_tool(
        &server,
        DEVELOPER,
        session.as_deref(),
        "get_certificates",
        json!({}),
    )
    .await;
    structured(&before, false);

    fixture.duplicate_manifest_entry("SR-05");

    for (tool, arguments) in every_tool_call() {
        let reply = call_tool(&server, DEVELOPER, session.as_deref(), tool, arguments).await;
        let body = structured(&reply, true);
        assert_eq!(
            body["status"], "dataset_not_verifiable",
            "`{tool}` must refuse a dataset the verifier rejects: {reply}",
        );
        let message = rendered(&reply);
        assert!(
            message.contains("SR-05"),
            "`{tool}` must say which entry is repeated: {message}",
        );
        assert!(
            !message.contains(&fixture.dataset_dir().display().to_string()),
            "a refusal must not report where the server keeps its files: {message}",
        );
        for secret in ["MS-07", "48213900000", "percent_complete"] {
            assert!(
                !message.contains(secret),
                "`{tool}` leaked record data (`{secret}`) while refusing: {message}",
            );
        }
    }
}

/// One dataset-level fault, and what each role must be told about it.
#[derive(Clone, Copy)]
struct FaultCase {
    /// The r3 review's name for it.
    label: &'static str,
    /// How to put the fault into a fixture.
    break_dataset: fn(&Fixture),
    /// What names the file that broke the dataset. The developer sees this;
    /// the buyer must never see it.
    detail: &'static str,
    /// What kind of fault it is, in words that name nothing in the dataset.
    /// This is the whole of what a buyer is told.
    kind: &'static str,
}

/// R3: a refusal names no document the caller may not read.
///
/// A dataset-level fault names the file that broke it, verbatim from the
/// verifier. The r3 review removed CN-14's manifest entry and every buyer
/// reply — five tools, `get_certificates` on the same path, and both
/// resource methods — came back carrying "unsigned file:
/// (docs/contractor-note-CN-14.md)". `answers.rs` opens by saying a
/// document the caller may not see is left out silently, because "naming
/// it, even to say it was withheld, would tell a buyer that an internal
/// document exists". A refusal is not an exception to that rule; it is
/// where it is easiest to forget. B1, B2 and B4 of the review, with the
/// developer as the control: the operator who can fix the dataset is still
/// told which file broke it, so the detail is filtered and not deleted.
#[tokio::test]
async fn a_dataset_fault_names_no_document_the_caller_may_not_read() {
    // Nothing any of these three faults is about may reach a buyer: not the
    // file, not the doc_id, not the unit whose schedule it is, and not the
    // directory this dataset keeps its documents in.
    const WITHHELD: [&str; 9] = [
        "CN-14",
        "contractor-note",
        "payment-schedule",
        "0802",
        "docs/",
        ".md",
        "doc_id",
        "CERT-MEP",
        "Lumenfield",
    ];

    let cases = [
        FaultCase {
            label: "B1: CN-14's manifest entry removed",
            break_dataset: |fixture| fixture.remove_manifest_entry("CN-14"),
            detail: "docs/contractor-note-CN-14.md",
            kind: "a file in the dataset that nothing signs",
        },
        FaultCase {
            label: "B2: PS-0802's manifest entry removed",
            break_dataset: |fixture| fixture.remove_manifest_entry("PS-0802"),
            detail: "docs/payment-schedule-unit-0802.md",
            kind: "a file in the dataset that nothing signs",
        },
        FaultCase {
            label: "B4: CN-14's manifest entry repeated",
            break_dataset: |fixture| fixture.duplicate_manifest_entry("CN-14"),
            detail: "CN-14",
            kind: "a manifest this verifier cannot act on",
        },
    ];

    for FaultCase {
        label: case,
        break_dataset,
        detail,
        kind,
    } in cases
    {
        let fixture = Fixture::new("dataset-fault");
        let server = TestServer::start(fixture.config()).await;
        let buyer_session = session_for(&server, BUYER).await;
        let developer_session = session_for(&server, DEVELOPER).await;

        // The fixture answers before the fault, so what the asserts below
        // see is the fault and not a broken copy.
        let before = call_tool(
            &server,
            BUYER,
            buyer_session.as_deref(),
            "get_milestones",
            json!({}),
        )
        .await;
        structured(&before, false);

        break_dataset(&fixture);

        for (tool, arguments) in every_tool_call() {
            let reply = call_tool(&server, BUYER, buyer_session.as_deref(), tool, arguments).await;
            let body = structured(&reply, true);
            assert_eq!(
                body["status"], "dataset_not_verifiable",
                "{case}: `{tool}` must refuse: {reply}",
            );
            let whole = rendered(&reply);
            for withheld in WITHHELD {
                assert!(
                    !whole.contains(withheld),
                    "{case}: `{tool}` told a buyer `{withheld}`: {whole}",
                );
            }
            let message = body["message"]
                .as_str()
                .unwrap_or_else(|| panic!("{case}: `{tool}` refused without saying why: {reply}"));
            assert!(
                message.contains(kind) && message.contains(" fault"),
                "{case}: `{tool}` must give a buyer the count and the kind of fault: {message}",
            );
        }

        // Both resource methods come through the same refusal.
        for request in [
            json!({"jsonrpc": "2.0", "id": next_id(), "method": "resources/list"}),
            json!({
                "jsonrpc": "2.0",
                "id": next_id(),
                "method": "resources/read",
                "params": {"uri": "ui://ask-aunova/evidence/PS-1204"}
            }),
        ] {
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            let response = server.post(BUYER, buyer_session.as_deref(), &request).await;
            let reply = message(response).await;
            let error = reply["error"]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("{case}: `{method}` must refuse: {reply}"));
            for withheld in WITHHELD {
                assert!(
                    !error.contains(withheld),
                    "{case}: `{method}` told a buyer `{withheld}`: {error}",
                );
            }
            assert!(
                error.contains(kind),
                "{case}: `{method}` must give a buyer the kind of fault: {error}",
            );
        }

        // The control: the detail is filtered by role, not deleted. The
        // developer is the operator here and is the one who can fix it.
        let operator = call_tool(
            &server,
            DEVELOPER,
            developer_session.as_deref(),
            "get_milestones",
            json!({}),
        )
        .await;
        let operator_body = structured(&operator, true);
        assert_eq!(
            operator_body["status"], "dataset_not_verifiable",
            "{case}: {operator}",
        );
        let operator_message = operator_body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("{case}: {operator}"));
        assert!(
            operator_message.contains(detail),
            "{case}: the developer must still be told which file broke the dataset \
             (`{detail}`): {operator_message}",
        );
    }
}

/// One way to make a *signed* record fail to parse, and what each role must
/// be told about it.
#[derive(Clone, Copy)]
struct ParseCase {
    /// The r4 review's name for it.
    label: &'static str,
    /// The edit, made to unit 0802's entry and nobody else's, before the
    /// fixture is re-signed with its own throwaway keys.
    break_record: fn(&Fixture),
    /// A fragment of what serde reported, which the developer must still be
    /// shown. The buyer must see none of it.
    detail: &'static str,
}

/// R1: a record that verifies but does not parse quotes no unit to a buyer.
///
/// The record is signed, so verification passes and `verify-dataset` exits 0
/// — no gate catches this. Then `serde_json` refuses it, and serde's error
/// quotes the value it tripped on with its line and column. The r4 review put
/// a type slip in unit 0802's entry and buyer:1204 was told 0802's
/// `paid_to_date_minor` in 18 of 36 reply bodies, across every tool and both
/// resource methods. This is r3's R3 one variant over: a dataset-level fault
/// reaching a caller through refusal text, on the variant that fix did not
/// cover. The developer is the control, so the detail is filtered and not
/// deleted.
#[tokio::test]
async fn a_signed_record_that_does_not_parse_quotes_no_other_unit_to_a_buyer() {
    // Nothing about unit 0802 may reach a buyer of 1204: not the figure in
    // minor units, not the same figure formatted, not the unit, not its
    // documents or its buyer reference — and not serde's own phrasing, which
    // is how all of it travelled.
    const WITHHELD: [&str; 14] = [
        "89100000",
        "891000.00",
        "891,000",
        "198000000",
        "0802",
        "PS-0802",
        "BUYER-PV-0802",
        "payment-schedule",
        "invalid type",
        "expected",
        "at line",
        "column",
        "did not parse",
        "1-bedroom",
    ];

    let cases = [
        ParseCase {
            label: "v1: 0802's paid_to_date_minor as a string",
            break_record: |fixture| {
                fixture.replace_once(
                    "project.json",
                    "\"paid_to_date_minor\": 89100000",
                    "\"paid_to_date_minor\": \"89100000\"",
                );
            },
            detail: "89100000",
        },
        ParseCase {
            label: "v2: 0802's paid_to_date_minor as a float",
            break_record: |fixture| {
                fixture.replace_once(
                    "project.json",
                    "\"paid_to_date_minor\": 89100000",
                    "\"paid_to_date_minor\": 89100000.5",
                );
            },
            detail: "89100000.5",
        },
        ParseCase {
            label: "v3: 0802's unit number as an integer",
            break_record: |fixture| {
                fixture.replace_once("project.json", "\"unit\": \"0802\"", "\"unit\": 802");
            },
            detail: "802",
        },
    ];

    for ParseCase {
        label: case,
        break_record,
        detail,
    } in cases
    {
        let fixture = Fixture::new("parse-leak");
        let server = TestServer::start(fixture.config()).await;
        let buyer_session = session_for(&server, BUYER).await;
        let developer_session = session_for(&server, DEVELOPER).await;
        // Give the fixture its own keys while the record still parses.
        fixture.re_sign();

        // The fixture answers before the edit, so what the asserts below see
        // is the parse failure and not a broken copy.
        let before = call_tool(
            &server,
            BUYER,
            buyer_session.as_deref(),
            "get_milestones",
            json!({}),
        )
        .await;
        structured(&before, false);

        // Signed, so every hash and signature still checks out: the tampering
        // is inside the record's JSON, not around it. `verify-dataset` now
        // rejects this at its own parse, so it is not "a record the verifier
        // accepts" — it is one whose signatures are valid and whose contents
        // `serde_json` refuses.
        // Signed over the raw bytes, because `re_sign` itself refuses to sign it.
        break_record(&fixture);
        fixture.re_sign_entry("PROJECT-RECORD");

        for (tool, arguments) in every_tool_call() {
            let reply = call_tool(&server, BUYER, buyer_session.as_deref(), tool, arguments).await;
            let body = structured(&reply, true);
            assert_eq!(
                body["status"], "dataset_unreadable",
                "{case}: `{tool}` must refuse a record that does not parse: {reply}",
            );
            let whole = rendered(&reply);
            for withheld in WITHHELD {
                assert!(
                    !whole.contains(withheld),
                    "{case}: `{tool}` told a buyer `{withheld}`: {whole}",
                );
            }
            let message = body["message"]
                .as_str()
                .unwrap_or_else(|| panic!("{case}: `{tool}` refused without saying why: {reply}"));
            assert!(
                message.contains("the signed dataset could not be read"),
                "{case}: `{tool}` must still tell a buyer the kind of failure: {message}",
            );
        }

        // Both resource methods come through the same refusal.
        for request in [
            json!({"jsonrpc": "2.0", "id": next_id(), "method": "resources/list"}),
            json!({
                "jsonrpc": "2.0",
                "id": next_id(),
                "method": "resources/read",
                "params": {"uri": "ui://ask-aunova/evidence/PS-1204"}
            }),
        ] {
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            let response = server.post(BUYER, buyer_session.as_deref(), &request).await;
            let reply = message(response).await;
            let error = reply["error"]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("{case}: `{method}` must refuse: {reply}"));
            for withheld in WITHHELD {
                assert!(
                    !error.contains(withheld),
                    "{case}: `{method}` told a buyer `{withheld}`: {error}",
                );
            }
            assert!(
                error.contains("the signed dataset could not be read"),
                "{case}: `{method}` must still name the kind of failure: {error}",
            );
        }

        // The control: the detail is filtered by role, not deleted. The
        // developer is the operator and is the one who can fix the record.
        let operator = call_tool(
            &server,
            DEVELOPER,
            developer_session.as_deref(),
            "get_payment_status",
            json!({"unit": "1204"}),
        )
        .await;
        let operator_body = structured(&operator, true);
        assert_eq!(
            operator_body["status"], "dataset_unreadable",
            "{case}: {operator}",
        );
        let operator_message = operator_body["message"]
            .as_str()
            .unwrap_or_else(|| panic!("{case}: {operator}"));
        assert!(
            operator_message.contains(detail),
            "{case}: the developer must still be told what would not parse (`{detail}`): \
             {operator_message}",
        );
    }
}

/// A `doc_id` the record cites but does not declare is named to nobody but the
/// developer.
///
/// The dangling id has no declaration, so it has no `visibility` either, and
/// the branch that handles it cannot ask the question every other document is
/// filtered by. Before this was settled it told everybody: a signed record
/// citing `CN-14` from a milestone every role may hear about put that name in
/// a buyer's `unverified` list, which is r3's R3 again — a buyer learning that
/// an internal document exists, this time through the one path that has no
/// visibility to check. The dataset is signed and otherwise sound: every
/// remaining hash and signature checks out, and every answer still comes back,
/// which is what makes this the answer path rather than a refusal.
#[tokio::test]
async fn a_cited_but_undeclared_document_is_named_to_nobody_but_the_developer() {
    // The id itself, the file it used to be, and the sentence that explains
    // it. None of them is a buyer's.
    const WITHHELD: [&str; 4] = [
        "CN-14",
        "contractor-note",
        "does not declare it",
        "no signature was checked",
    ];

    let fixture = Fixture::new("undeclared-citation");
    let server = TestServer::start(fixture.config()).await;
    let buyer_session = session_for(&server, BUYER).await;
    let developer_session = session_for(&server, DEVELOPER).await;

    let before = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    structured(&before, false);

    // MS-07 is visible to every role, and the id it now cites is declared by
    // nothing and signed by nothing.
    fixture.cite_without_declaring(CONTRACTOR_NOTE_ID, "MS-07");
    fixture.re_sign();

    for (tool, arguments) in every_tool_call() {
        let reply = call_tool(&server, BUYER, buyer_session.as_deref(), tool, arguments).await;
        let whole = rendered(&reply);
        for withheld in WITHHELD {
            assert!(
                !whole.contains(withheld),
                "`{tool}` told a buyer `{withheld}`: {whole}",
            );
        }
    }

    // The milestone that cites it is still answered, from the evidence that
    // did verify: this is the answer path, not a refusal that happens to name
    // nothing.
    let answered = call_tool(
        &server,
        BUYER,
        buyer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let answered_body = structured(&answered, false);
    assert!(
        answered_body["milestones"]
            .as_array()
            .is_some_and(|milestones| milestones
                .iter()
                .any(|milestone| milestone["id"] == "MS-07")),
        "the milestone citing the undeclared document must still be answered from the \
         evidence that did verify: {answered}",
    );

    // Both resource methods, which compose their cards from the same index.
    for request in [
        json!({"jsonrpc": "2.0", "id": next_id(), "method": "resources/list"}),
        json!({
            "jsonrpc": "2.0",
            "id": next_id(),
            "method": "resources/read",
            "params": {"uri": "ui://ask-aunova/evidence/PS-1204"}
        }),
    ] {
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let response = server.post(BUYER, buyer_session.as_deref(), &request).await;
        let reply = message(response).await;
        let whole = rendered(&reply);
        for withheld in WITHHELD {
            assert!(
                !whole.contains(withheld),
                "`{method}` told a buyer `{withheld}`: {whole}",
            );
        }
    }

    // The control: the developer is the operator who can fix the record, and
    // is told exactly which id dangles and why it evidences nothing.
    let operator = call_tool(
        &server,
        DEVELOPER,
        developer_session.as_deref(),
        "get_milestones",
        json!({}),
    )
    .await;
    let operator_body = structured(&operator, false);
    let dangling = operator_body["unverified"]
        .as_array()
        .unwrap_or_else(|| panic!("an answer carries its unverified documents: {operator}"))
        .iter()
        .find(|document| document["doc_id"] == CONTRACTOR_NOTE_ID)
        .unwrap_or_else(|| {
            panic!("the developer must be told which cited document dangles: {operator}")
        });
    assert_eq!(dangling["status"], "missing", "{dangling}");
    assert!(
        dangling["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("does not declare it")),
        "the developer must be told why it evidences nothing: {dangling}",
    );
}
