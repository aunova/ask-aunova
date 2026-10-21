//! The loopback HTTP face: the page, and the small JSON API it calls.
//!
//! The browser can reach four things and nothing else: the page, what this
//! build is (`/api/about`), one question (`/api/ask`) and one evidence card
//! (`/api/evidence`). Each of the last three opens its own MCP session, does
//! the real `initialize` handshake, and closes it again — so the drawer the
//! page shows is a record of an exchange that actually happened rather than a
//! replay.
//!
//! The identity the page sends chooses a bearer token and nothing else. It is
//! consumed in [`McpSession::connect`] and is not passed on, so no handler
//! below this line has a role to branch on and none of them filters anything.
//! The server decides, every time.
//!
//! Both reads from the browser are bounded before anything is sent onward: a
//! question longer than [`MAX_QUESTION_CHARS`], or a document id that is not a
//! plain identifier, is refused here rather than concatenated into a `ui://`
//! URI.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    endpoint::Endpoint,
    identity::Identity,
    mcp::{McpError, McpSession},
    model::Model,
    page::{PAGE_CONTENT_TYPE, PAGE_HTML},
    stub::{SCRIPTED_QUESTIONS, ScriptedQuestion},
    transcribe::{MAX_RECORDING_BYTES, Recording, TranscribeError, Transcriber},
    turn::{self, Turn, TurnError},
    unit_id::{self, Interpretation},
    voice::Voice,
};

/// Longest question this simulator will carry to the server.
///
/// A spoken question is a sentence. This is far above that and well below
/// anything that would make a tool call unwieldy; the point is that the bound
/// exists and is stated, not where it sits.
pub const MAX_QUESTION_CHARS: usize = 500;

/// Longest document id this simulator will ask for a card for.
pub const MAX_DOC_ID_CHARS: usize = 64;

/// Most alternatives this simulator will read from one recognition.
///
/// The page asks its recogniser for five. This is above that and bounded:
/// the alternatives come from a browser, which is whoever is driving it.
pub const MAX_ALTERNATIVES: usize = 8;

/// Everything a request needs: where the server is, and what answers.
pub struct Simulator {
    endpoint: Endpoint,
    model: Box<dyn Model>,
    voice: Box<dyn Voice>,
    transcriber: Box<dyn Transcriber>,
}

impl Simulator {
    /// Assemble the simulator from the endpoint it talks to and the three
    /// implementations it runs on.
    pub fn new(
        endpoint: Endpoint,
        model: Box<dyn Model>,
        voice: Box<dyn Voice>,
        transcriber: Box<dyn Transcriber>,
    ) -> Self {
        Self {
            endpoint,
            model,
            voice,
            transcriber,
        }
    }

    /// The MCP endpoint every session is opened against.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Run one question, opening and closing its own MCP session.
    ///
    /// Public because the acceptance tests drive exactly this, over real HTTP
    /// against the real server, rather than a copy of it.
    pub async fn ask(&self, identity: Identity, question: &str) -> Result<Turn, ApiError> {
        let question = question.trim();
        if question.is_empty() {
            return Err(ApiError::QuestionEmpty);
        }
        let chars = question.chars().count();
        if chars > MAX_QUESTION_CHARS {
            return Err(ApiError::QuestionTooLong { chars });
        }

        let session = McpSession::connect(&self.endpoint, identity).await?;
        let answered =
            turn::run_turn(&session, self.model.as_ref(), self.voice.as_ref(), question).await;
        // Closed whichever way the turn went: a failed answer is not a reason
        // to leave a session open on the server.
        session.close().await;
        Ok(answered?)
    }

    /// Read one evidence card over MCP, in its own session.
    pub async fn evidence(&self, identity: Identity, doc_id: &str) -> Result<String, ApiError> {
        let doc_id = checked_doc_id(doc_id)?;
        let session = McpSession::connect(&self.endpoint, identity).await?;
        let card = session.read_evidence_card(doc_id).await;
        session.close().await;
        Ok(card?)
    }

    /// Transcribe one recording, and read what it says.
    ///
    /// Two steps, one path: whatever produced the words, they are read by
    /// the same normaliser the browser's own recogniser is read by, and
    /// neither of them reaches a tool call without a person confirming what
    /// is in here first.
    pub async fn heard(&self, recording: Recording) -> Result<Heard, ApiError> {
        let transcript = self.transcriber.transcribe(&recording).await?;
        Ok(Heard {
            note: transcript.note,
            interpretation: unit_id::interpret(&transcript.alternatives),
        })
    }

    /// Read what a recogniser in the browser heard.
    ///
    /// The alternatives come from a browser and are bounded here, before the
    /// normaliser sees them.
    pub fn interpret(&self, heard: Vec<String>) -> Result<Interpretation, ApiError> {
        if heard.len() > MAX_ALTERNATIVES {
            return Err(ApiError::TooManyAlternatives { count: heard.len() });
        }
        for alternative in &heard {
            let chars = alternative.chars().count();
            if chars > MAX_QUESTION_CHARS {
                return Err(ApiError::QuestionTooLong { chars });
            }
        }
        Ok(unit_id::interpret(&heard))
    }
}

/// Why a request could not be answered.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The MCP exchange failed before any answer existed.
    #[error(transparent)]
    Mcp(#[from] McpError),
    /// The loop could not complete the turn.
    #[error(transparent)]
    Turn(#[from] TurnError),
    /// Nothing was asked.
    #[error("a question with nothing in it has nothing to ask")]
    QuestionEmpty,
    /// More than this simulator will carry.
    #[error(
        "a question may be at most {MAX_QUESTION_CHARS} characters; this one is {chars}, so \
         nothing was sent"
    )]
    QuestionTooLong {
        /// How long it was.
        chars: usize,
    },
    /// Not a document id, so not something to build a resource URI out of.
    #[error(
        "`{clipped}` is not a document id: this client asks for cards by identifier only, at \
         most {MAX_DOC_ID_CHARS} characters of letters, digits, `.`, `_` or `-`"
    )]
    DocId {
        /// As much of what was sent as is worth repeating.
        clipped: String,
    },
    /// More alternatives than one recognition can honestly have offered.
    #[error(
        "at most {MAX_ALTERNATIVES} alternatives are read from one recognition; this request \
         carried {count}, so none of them were"
    )]
    TooManyAlternatives {
        /// How many arrived.
        count: usize,
    },
    /// The recording could not be turned into words.
    #[error(transparent)]
    Transcribe(#[from] TranscribeError),
}

impl ApiError {
    /// What this failure is, in HTTP's vocabulary.
    ///
    /// A question the model has no rule for is the model saying so, not the
    /// server failing, so the two are not the same status.
    fn status(&self) -> StatusCode {
        match self {
            ApiError::QuestionEmpty
            | ApiError::QuestionTooLong { .. }
            | ApiError::DocId { .. }
            | ApiError::TooManyAlternatives { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            // What arrived was not a recording this build can transcribe,
            // which is this request's problem; a live transcriber that could
            // not be reached is not.
            ApiError::Transcribe(
                TranscribeError::Empty
                | TranscribeError::TooLong { .. }
                | TranscribeError::NotAligned { .. }
                | TranscribeError::NoFixture { .. },
            ) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Transcribe(
                TranscribeError::NotConfigured { .. } | TranscribeError::Service { .. },
            ) => StatusCode::BAD_GATEWAY,
            ApiError::Turn(TurnError::Model(_)) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Mcp(_) | ApiError::Turn(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        // Every message this can carry was built by this crate from a
        // validated endpoint and the server's own words. No transport error
        // is formatted anywhere in it, so no request URL can be in it.
        (
            status,
            Json(ApiFailure {
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

/// The body of every failed request.
#[derive(Debug, Serialize)]
struct ApiFailure {
    /// What went wrong, in one sentence.
    error: String,
}

/// What this build is, for the page's switcher, chips and footer.
#[derive(Debug, Serialize)]
struct About {
    /// This client's name and version, as sent in `initialize`.
    client: String,
    /// The MCP endpoint every session is opened against.
    endpoint: String,
    /// Which model implementation is answering.
    model: String,
    /// Whether that model sends the question off this machine.
    model_is_live: bool,
    /// Which voice implementation is speaking.
    voice: String,
    /// Whether that voice sends the answer text off this machine.
    voice_is_live: bool,
    /// Which transcriber is turning recordings into words.
    transcriber: String,
    /// Whether that transcriber sends the recording off this machine.
    ///
    /// Sent as a flag rather than left for the page to infer from
    /// `transcriber`, which is a sentence written for a human. A page that
    /// pattern-matches prose to decide where somebody's voice goes is one
    /// rewording away from being wrong about it.
    transcriber_is_live: bool,
    /// The identities the switcher offers.
    identities: Vec<IdentityChoice>,
    /// The questions the model is scripted for.
    scripted_questions: &'static [ScriptedQuestion],
}

/// One entry in the role switcher.
#[derive(Debug, Serialize)]
struct IdentityChoice {
    /// What the page sends back to choose it.
    key: &'static str,
    /// What the page shows.
    label: &'static str,
}

/// One question from the page.
#[derive(Debug, Deserialize)]
struct AskRequest {
    /// Which demo identity to authenticate as.
    identity: Identity,
    /// What was asked.
    question: String,
}

/// What a recogniser in the browser heard, as the page sends it.
#[derive(Debug, Deserialize)]
struct InterpretRequest {
    /// Every alternative the recogniser offered, best first.
    heard: Vec<String>,
}

/// One recording, transcribed and read.
#[derive(Debug, Serialize)]
pub struct Heard {
    /// What the transcriber did, in one sentence, for the page to show.
    pub note: String,
    /// What the words were taken to mean, and what may be asked next.
    pub interpretation: Interpretation,
}

/// One evidence card request from the page, read out of the request path.
#[derive(Debug, Deserialize)]
struct EvidenceRequest {
    /// Which demo identity to authenticate as.
    identity: Identity,
    /// The document whose card to read.
    doc_id: String,
}

/// One evidence card, as the server rendered it.
#[derive(Debug, Serialize)]
struct EvidenceCard {
    /// The document it evidences.
    doc_id: String,
    /// The card's HTML, which the page puts in a sandboxed frame.
    html: String,
}

/// The whole HTTP surface of this binary.
pub fn app(simulator: Arc<Simulator>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/api/about", get(about))
        .route("/api/ask", post(ask))
        .route(
            "/api/transcribe",
            // The one route that takes bytes rather than JSON, and the only
            // one with a body worth bounding at the layer: a body past the
            // cap is rejected here, whole, rather than read and cut down.
            post(transcribe).layer(DefaultBodyLimit::max(MAX_RECORDING_BYTES)),
        )
        .route("/api/interpret", post(interpret))
        .route("/api/evidence/{identity}/{doc_id}", get(evidence))
        .with_state(simulator)
}

/// Serve the page, from the binary.
async fn page() -> Response {
    ([(header::CONTENT_TYPE, PAGE_CONTENT_TYPE)], PAGE_HTML).into_response()
}

/// Say what this build is.
async fn about(State(simulator): State<Arc<Simulator>>) -> Json<About> {
    Json(About {
        client: format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        endpoint: simulator.endpoint.url().to_owned(),
        model: simulator.model.name().to_owned(),
        model_is_live: simulator.model.sends_questions_off_this_machine(),
        voice: simulator.voice.name().to_owned(),
        voice_is_live: simulator.voice.sends_text_off_this_machine(),
        transcriber: simulator.transcriber.name().to_owned(),
        transcriber_is_live: simulator.transcriber.sends_recordings_off_this_machine(),
        identities: Identity::ALL
            .iter()
            .map(|identity| IdentityChoice {
                key: identity.key(),
                label: identity.label(),
            })
            .collect(),
        scripted_questions: &SCRIPTED_QUESTIONS,
    })
}

/// Answer one question.
async fn ask(
    State(simulator): State<Arc<Simulator>>,
    Json(request): Json<AskRequest>,
) -> Result<Json<Turn>, ApiError> {
    Ok(Json(
        simulator.ask(request.identity, &request.question).await?,
    ))
}

/// Transcribe one recording the page captured.
///
/// The body is raw 16 kHz mono signed 16-bit little-endian PCM — the samples
/// the page's worklet produced, with no container around them. Nothing is
/// asked of the MCP server here: this returns what was heard and what it was
/// taken to mean, and the page shows both before anything is asked.
async fn transcribe(
    State(simulator): State<Arc<Simulator>>,
    body: Bytes,
) -> Result<Json<Heard>, ApiError> {
    Ok(Json(simulator.heard(Recording::new(body.into())?).await?))
}

/// Read what the browser's own recogniser heard.
///
/// The same normaliser, and the same confirmation afterwards, as the
/// recording path: two ways of producing words, one way of reading them.
async fn interpret(
    State(simulator): State<Arc<Simulator>>,
    Json(request): Json<InterpretRequest>,
) -> Result<Json<Interpretation>, ApiError> {
    Ok(Json(simulator.interpret(request.heard)?))
}

/// Read one evidence card.
///
/// Addressed by path rather than by query string: a card is one resource with
/// one id, so `/api/evidence/<identity>/<doc_id>` says what it is, and
/// reading it needs no query-string parser in the dependency tree.
async fn evidence(
    State(simulator): State<Arc<Simulator>>,
    Path(request): Path<EvidenceRequest>,
) -> Result<Json<EvidenceCard>, ApiError> {
    let html = simulator
        .evidence(request.identity, &request.doc_id)
        .await?;
    Ok(Json(EvidenceCard {
        doc_id: request.doc_id,
        html,
    }))
}

/// A document id, or a refusal that does not build a URI out of it.
///
/// The page only ever sends ids the server itself cited, but it is a browser
/// and the query string is whoever is driving it. A `ui://` URI is built by
/// concatenation, so what may be concatenated is decided here.
fn checked_doc_id(doc_id: &str) -> Result<&str, ApiError> {
    let acceptable = !doc_id.is_empty()
        && doc_id.chars().count() <= MAX_DOC_ID_CHARS
        && doc_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character));
    if acceptable {
        return Ok(doc_id);
    }
    Err(ApiError::DocId {
        clipped: clipped(doc_id),
    })
}

/// As much of a caller's own string as is worth repeating, cut on a character
/// boundary so a non-ASCII byte cannot panic the cut.
fn clipped(value: &str) -> String {
    let mut clipped: String = value.chars().take(MAX_DOC_ID_CHARS).collect();
    if value.chars().nth(MAX_DOC_ID_CHARS).is_some() {
        clipped.push('…');
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_document_id_is_accepted() {
        for doc_id in ["SR-07", "CERT-STRUCT-2026-114", "PROJECT-RECORD", "PS-1204"] {
            assert_eq!(checked_doc_id(doc_id).ok(), Some(doc_id));
        }
    }

    #[test]
    fn nothing_that_could_steer_a_uri_is_accepted() {
        for refused in [
            "",
            "../../etc/passwd",
            "SR-07/../PS-1510",
            "ui://ask-aunova/evidence/SR-07",
            "SR 07",
            "SR-07?x=1",
        ] {
            assert!(
                checked_doc_id(refused).is_err(),
                "`{refused}` must not be concatenated into a resource URI",
            );
        }
    }

    #[test]
    fn an_over_long_document_id_is_refused_and_clipped_on_a_character_boundary() {
        let long = "é".repeat(MAX_DOC_ID_CHARS * 2);
        let error = checked_doc_id(&long).expect_err("it is past the cap");
        let message = error.to_string();
        assert!(message.contains('…'), "{message}");
        assert_eq!(
            message.matches('é').count(),
            MAX_DOC_ID_CHARS,
            "exactly the cap is repeated, and slicing bytes would have panicked here",
        );
    }

    #[test]
    fn a_model_with_no_rule_is_not_reported_as_a_server_failure() {
        let no_rule = ApiError::Turn(TurnError::Model(crate::model::ModelError::NoRule {
            rounds: 0,
        }));
        assert_eq!(no_rule.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let unreachable = ApiError::Mcp(McpError::Initialize {
            endpoint: "http://127.0.0.1:8731/mcp".to_owned(),
            detail: "it did not answer within the handshake deadline",
        });
        assert_eq!(unreachable.status(), StatusCode::BAD_GATEWAY);
    }
}
