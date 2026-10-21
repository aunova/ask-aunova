//! The tool-use loop running on a hosted model, through Bedrock's Converse API.
//!
//! **Nothing in the default build constructs this.** It compiles only under
//! the `live` feature and is reached only when the binary is started with
//! `--live`; every test runs on [`StubModel`](crate::stub::StubModel).
//!
//! **The model is never trusted with a fact.** It is given the question and
//! the tool schemas the server advertised, it chooses calls, and it composes
//! an answer out of sentences it says it copied from one named tool result.
//! [`crate::turn`] then checks every one of those sentences against the
//! recorded result before anything is spoken. So a model that writes a fluent
//! sentence from its own knowledge produces an answer with nothing in it,
//! rather than a confident fabrication — the loop fails closed, and this
//! module does not have to be right for that to hold.
//!
//! **The exchange is rebuilt from scratch on every step.** [`Model`] is
//! stateless by design: the turn hands over the question and every outcome so
//! far, and this module replays them as a conversation. That means there is no
//! hidden state in which a fact could survive a round, and the tool-use ids it
//! sends are derived from the outcome index rather than remembered.

use aws_sdk_bedrockruntime::{
    Client,
    types::{
        ContentBlock, ConversationRole, ConverseOutput, InferenceConfiguration, Message,
        SystemContentBlock, Tool, ToolConfiguration, ToolInputSchema, ToolResultBlock,
        ToolResultContentBlock, ToolSpecification, ToolUseBlock,
    },
};
use aws_smithy_types::Document;
use serde_json::{Map, Value};

use crate::model::{
    Draft, Model, ModelError, SYSTEM_PROMPT, Step, Stepping, ToolOutcome, ToolSpec, ToolUse,
};

/// The model this demo runs on.
///
/// The in-region id, not a `us.` inference profile: the IAM policy for this
/// demo allows one region, and a cross-region profile would be denied.
pub const MODEL_ID: &str = "amazon.nova-pro-v1:0";

/// Most output tokens one call may produce.
///
/// Sent on every call, because a request without it reserves the model's
/// whole output budget against the account's daily token quota whether or not
/// the reply uses it — so an unset field is not a neutral default, it is the
/// most expensive possible one.
///
/// 512 is sized for what this model is actually asked for: a short JSON
/// object of sentences copied out of tool results, or a tool call. It is not
/// sized for prose, and it should not be: a reply long enough to be truncated
/// here is a reply that stopped quoting and started composing.
const MAX_OUTPUT_TOKENS: i32 = 512;

/// Temperature zero: the model is choosing tools and copying sentences out of
/// their results. Nothing in that is better for being varied, and a demo that
/// answers differently each time is harder to trust and harder to film.
const TEMPERATURE: f32 = 0.0;

/// Appended to [`SYSTEM_PROMPT`], and the only part of the prompt that is
/// about this transport rather than about honesty.
const ANSWER_PROTOCOL: &str = "\n\nWhen you have everything you need, stop calling tools and \
reply with one JSON object and no \
other text:

{\"statements\":[{\"from_call\":0,\"quoted\":\"a sentence copied verbatim from that call's \
result\",\"citations\":[\"DOC-ID\"],\"standing\":\"verified\"}]}

`from_call` is the index of the tool call you took the sentence from, counting from zero in the \
order they were made. `standing` is one of verified, contradicted, unverified, refused, and must \
match what that result said. Every sentence is checked against the recorded result before it is \
spoken, and any sentence that does not match it is discarded, so inventing one costs you the \
sentence and gains nothing.";

/// The Bedrock client configuration, with the environment's bearer token
/// deliberately refused.
///
/// `aws-sdk-bedrockruntime` reads `AWS_BEARER_TOKEN_BEDROCK` and, when it is
/// set, switches the client to bearer authentication — signing as whoever
/// owns that token instead of the profile this binary was told to use. That
/// is the same defect as the environment access key the credentials provider
/// is pinned against (see [`crate::aws`]), reached through a different door:
/// nothing about naming a profile closes it, because it is not a credentials
/// question at all, it is an auth-scheme one.
///
/// **One bolt holds this, not two.** `set_token_provider(None)` is a no-op
/// in `aws-sdk-bedrockruntime` 1.148.0 — the setter only acts on `Some` — so
/// the protection is the auth scheme preference alone. Measured, not
/// assumed: with the token cleared and the preference left at its default,
/// the config still resolves to `httpBearerAuth`. The call is kept because
/// it is what a later SDK would need if that setter ever starts clearing,
/// and it is documented as doing nothing today so that nobody counts it as
/// a second defence.
///
/// Bedrock is the only one of the three services with this hook. Neither
/// `aws_sdk_polly::config::Builder` nor
/// `aws_sdk_transcribestreaming::config::Builder` has a `set_token_provider`
/// at all — calling it on either is a compile error, which is a stronger
/// check than reading their source, and the reason there is nothing to clear
/// in the voice or the transcriber.
///
/// Public so that a test in its own process can export a bearer token and
/// assert the client still resolves SigV4.
pub fn bedrock_config(shared: &aws_config::SdkConfig) -> aws_sdk_bedrockruntime::Config {
    let mut builder = aws_sdk_bedrockruntime::config::Builder::from(shared);
    builder.set_token_provider(None);
    builder
        .auth_scheme_preference([aws_runtime::auth::sigv4::SCHEME_ID])
        .build()
}

/// A model that answers through Bedrock's Converse API.
#[derive(Clone, Debug)]
pub struct BedrockModel {
    client: Client,
    model_id: String,
    prompt: String,
}

impl BedrockModel {
    /// Build a model from the shared profile configuration.
    ///
    /// Constructing a client makes no request: credentials are resolved when
    /// the first call is made, not here.
    pub async fn new() -> Result<Self, crate::aws::AwsError> {
        let config = crate::aws::config().await?;
        Ok(Self {
            client: Client::from_conf(bedrock_config(&config)),
            model_id: MODEL_ID.to_owned(),
            prompt: format!("{SYSTEM_PROMPT}{ANSWER_PROTOCOL}"),
        })
    }

    /// The tool-use id for the call at `index`.
    ///
    /// Derived rather than remembered, because the trait is stateless and the
    /// conversation is rebuilt each step.
    fn call_id(index: usize) -> String {
        format!("call-{index}")
    }

    /// Replay the question and everything seen so far as a conversation.
    fn conversation(question: &str, seen: &[ToolOutcome]) -> Result<Vec<Message>, ModelError> {
        let mut messages = vec![message(
            ConversationRole::User,
            ContentBlock::Text(question.to_owned()),
        )?];

        for outcome in seen {
            let use_block = ToolUseBlock::builder()
                .tool_use_id(Self::call_id(outcome.index))
                .name(&outcome.tool)
                .input(to_document(&outcome.arguments))
                .build()
                .map_err(build_failed)?;
            messages.push(message(
                ConversationRole::Assistant,
                ContentBlock::ToolUse(use_block),
            )?);

            let result_block = ToolResultBlock::builder()
                .tool_use_id(Self::call_id(outcome.index))
                .content(ToolResultContentBlock::Json(to_document(&outcome.result)))
                .status(if outcome.refused {
                    aws_sdk_bedrockruntime::types::ToolResultStatus::Error
                } else {
                    aws_sdk_bedrockruntime::types::ToolResultStatus::Success
                })
                .build()
                .map_err(build_failed)?;
            messages.push(message(
                ConversationRole::User,
                ContentBlock::ToolResult(result_block),
            )?);
        }

        Ok(messages)
    }

    /// The bounds every call is made under.
    ///
    /// Built per call rather than held on the struct: it is two fields, and a
    /// cached copy would be one more thing that can silently stop matching
    /// the constants above.
    fn inference_config() -> InferenceConfiguration {
        InferenceConfiguration::builder()
            .max_tokens(MAX_OUTPUT_TOKENS)
            .temperature(TEMPERATURE)
            .build()
    }

    /// The advertised tools, in the shape Converse wants them.
    fn tool_config(tools: &[ToolSpec]) -> Result<ToolConfiguration, ModelError> {
        let mut config = ToolConfiguration::builder();
        for tool in tools {
            let spec = ToolSpecification::builder()
                .name(&tool.name)
                .description(&tool.description)
                .input_schema(ToolInputSchema::Json(to_document(&tool.input_schema)))
                .build()
                .map_err(build_failed)?;
            config = config.tools(Tool::ToolSpec(spec));
        }
        config.build().map_err(build_failed)
    }

    /// Turn one Converse reply into a step.
    fn step_from(message: Message, tools: &[ToolSpec]) -> Result<Step, ModelError> {
        let mut calls = Vec::new();
        let mut text = String::new();

        for block in message.content {
            match block {
                ContentBlock::ToolUse(use_block) => {
                    let name = use_block.name().to_owned();
                    if !tools.iter().any(|tool| tool.name == name) {
                        return Err(ModelError::ToolNotAdvertised { tool: name });
                    }
                    calls.push(ToolUse {
                        tool: name,
                        arguments: from_document(use_block.input()),
                    });
                }
                ContentBlock::Text(said) => text.push_str(&said),
                // Converse can carry image, video, document and guard blocks.
                // None of them can be a fact about this project, so none of
                // them reaches the answer.
                _ => {}
            }
        }

        if !calls.is_empty() {
            return Ok(Step::UseTools(calls));
        }
        Ok(Step::Answer(parse_draft(&text)))
    }
}

impl Model for BedrockModel {
    fn name(&self) -> &str {
        "Amazon Nova Pro on Bedrock, choosing tools"
    }

    fn sends_questions_off_this_machine(&self) -> bool {
        true
    }

    fn system_prompt(&self) -> &str {
        &self.prompt
    }

    fn next_step<'a>(
        &'a self,
        question: &'a str,
        tools: &'a [ToolSpec],
        seen: &'a [ToolOutcome],
    ) -> Stepping<'a> {
        Box::pin(async move {
            let reply = self
                .client
                .converse()
                .model_id(&self.model_id)
                .system(SystemContentBlock::Text(self.prompt.clone()))
                .set_messages(Some(Self::conversation(question, seen)?))
                .tool_config(Self::tool_config(tools)?)
                .inference_config(Self::inference_config())
                .send()
                .await
                .map_err(|error| {
                    // Logged in full for the operator; never returned, because
                    // an SDK error's Display can carry the request URL.
                    //
                    // `DisplayErrorContext` rather than `{error}`: an
                    // `SdkError`'s own Display is the single word "service
                    // error", which cannot tell a throttle from a denial from
                    // a malformed request. The whole point of an operator log
                    // is to name which of those it was.
                    tracing::warn!(
                        "Bedrock refused the exchange: {}",
                        aws_smithy_types::error::display::DisplayErrorContext(&error)
                    );
                    ModelError::Unavailable {
                        detail: "the request was refused or could not be sent; what went wrong is \
                                 recorded where this simulator is run"
                            .to_owned(),
                    }
                })?;

            match reply.output {
                Some(ConverseOutput::Message(message)) => Self::step_from(message, tools),
                _ => Err(ModelError::Unavailable {
                    detail: "the reply carried no message".to_owned(),
                }),
            }
        })
    }
}

/// One conversation message carrying one block.
fn message(role: ConversationRole, block: ContentBlock) -> Result<Message, ModelError> {
    Message::builder()
        .role(role)
        .content(block)
        .build()
        .map_err(build_failed)
}

/// A builder that refused its own input is this crate's mistake, not the
/// service's, so it is reported as unavailable with a fixed phrase.
fn build_failed<E>(_error: E) -> ModelError {
    ModelError::Unavailable {
        detail: "this client could not assemble the request".to_owned(),
    }
}

/// Read a draft out of the model's final text.
///
/// Lenient on purpose. Every statement is checked against the recorded tool
/// result afterwards, so the cost of accepting junk here is an answer with
/// nothing in it — never a wrong fact. Returning an empty draft for
/// unparseable text is the same outcome by a shorter route.
fn parse_draft(text: &str) -> Draft {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|rest| rest.trim_start_matches('\n').trim_end_matches('`').trim())
        .unwrap_or(trimmed);

    let Some(start) = body.find('{') else {
        return Draft::default();
    };
    let Some(end) = body.rfind('}') else {
        return Draft::default();
    };
    serde_json::from_str(&body[start..=end]).unwrap_or_default()
}

/// `serde_json` to the SDK's own document type.
fn to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(it) => Document::Bool(*it),
        Value::Number(it) => it
            .as_i64()
            .map(|n| Document::Number(aws_smithy_types::Number::NegInt(n)))
            .or_else(|| {
                it.as_f64()
                    .map(|n| Document::Number(aws_smithy_types::Number::Float(n)))
            })
            .unwrap_or(Document::Null),
        Value::String(it) => Document::String(it.clone()),
        Value::Array(items) => Document::Array(items.iter().map(to_document).collect()),
        Value::Object(fields) => Document::Object(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), to_document(field)))
                .collect(),
        ),
    }
}

/// The SDK's document type back to `serde_json`, for the arguments a tool use
/// carried. Anything that is not an object is sent as no arguments at all,
/// which the server then refuses by schema rather than this client guessing.
fn from_document(document: &Document) -> Map<String, Value> {
    match to_value(document) {
        Value::Object(fields) => fields,
        _ => Map::new(),
    }
}

fn to_value(document: &Document) -> Value {
    match document {
        Document::Null => Value::Null,
        Document::Bool(it) => Value::Bool(*it),
        Document::Number(aws_smithy_types::Number::PosInt(n)) => Value::from(*n),
        Document::Number(aws_smithy_types::Number::NegInt(n)) => Value::from(*n),
        Document::Number(aws_smithy_types::Number::Float(n)) => Value::from(*n),
        Document::String(it) => Value::String(it.clone()),
        Document::Array(items) => Value::Array(items.iter().map(to_value).collect()),
        Document::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), to_value(field)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Standing;

    #[test]
    fn every_call_is_bounded_and_deterministic() {
        // A builder field that is quietly dropped leaves an `Option` at
        // `None`, and an absent `maxTokens` is not a neutral default: Bedrock
        // reserves the model's whole output budget against the daily quota
        // on every call. That costs real tokens and shows up as a throttle,
        // not as a failure anything here would catch.
        let config = BedrockModel::inference_config();
        assert_eq!(config.max_tokens(), Some(MAX_OUTPUT_TOKENS));
        assert_eq!(config.temperature(), Some(TEMPERATURE));
    }

    #[test]
    fn a_json_draft_is_read_and_a_fenced_one_too() {
        let bare = r#"{"statements":[{"from_call":1,"quoted":"Unit 1204 has paid.","citations":["PS-1204"],"standing":"verified"}]}"#;
        let draft = parse_draft(bare);
        assert_eq!(draft.statements.len(), 1);
        assert_eq!(draft.statements[0].from_call, 1);
        assert_eq!(draft.statements[0].standing, Standing::Verified);

        let fenced = format!("```json\n{bare}\n```");
        assert_eq!(parse_draft(&fenced), draft);

        let chatty = format!("Here is the answer.\n{bare}\nHope that helps.");
        assert_eq!(parse_draft(&chatty), draft);
    }

    #[test]
    fn prose_the_model_invented_becomes_an_empty_draft_not_a_sentence() {
        // The loop checks statements against recorded results, so the only
        // safe reading of text that is not a draft is "no statements".
        for said in [
            "Unit 1204 has paid one million four hundred seventy thousand dirhams.",
            "",
            "   ",
            "{ not json",
        ] {
            assert_eq!(
                parse_draft(said),
                Draft::default(),
                "free text must not become a statement: {said:?}",
            );
        }
    }

    #[test]
    fn json_round_trips_through_the_sdk_document_type() {
        let value: Value = serde_json::from_str(
            r#"{"unit":"1204","n":7,"ok":true,"none":null,"xs":[1,"two"],"deep":{"a":"b"}}"#,
        )
        .expect("the fixture is valid JSON");
        assert_eq!(to_value(&to_document(&value)), value);
    }

    #[test]
    fn the_model_id_is_the_in_region_one() {
        // A `us.` inference profile is a cross-region call, which this demo's
        // IAM policy denies; using one would fail at the call.
        assert!(
            !MODEL_ID.starts_with("us."),
            "a cross-region inference profile would be denied: {MODEL_ID}",
        );
    }
}
