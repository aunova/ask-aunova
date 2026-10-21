//! The model side of the loop: a Bedrock Converse-style tool-use exchange.
//!
//! The shape is the one a Converse API gives you — the model is handed the
//! question and the tool schemas the server advertised, it answers with either
//! tool uses or a final message, and tool results come back to it on the next
//! round. [`StubModel`](crate::stub::StubModel) is the implementation
//! everything runs on; the trait exists so a hosted model can be dropped in
//! without the loop around it changing.
//!
//! What the trait deliberately does *not* allow is a free-text answer. A
//! [`Draft`] is a list of [`Statement`]s, and a statement is a sentence the
//! model says it copied out of one named tool result, with the document ids
//! that result cited for it. The loop checks that claim against the recorded
//! result before anything is spoken, so a model that writes a fluent sentence
//! from its own knowledge produces an answer with nothing in it rather than a
//! confident fabrication. That check is in [`crate::turn`], not here: a model
//! cannot be trusted to mark its own homework.

use std::{future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[cfg(feature = "live")]
pub mod live;

/// The rules the loop obeys, in the form a hosted model would be given them.
///
/// This is shown in the page's debug drawer, so a viewer can read the rules
/// and then watch the tool calls that honour them. It is also what a live
/// implementation would send as its system prompt.
pub const SYSTEM_PROMPT: &str = "\
You answer questions about one building project by calling the tools of an MCP server that \
proves every fact against signed source documents. You have no knowledge of this project. \
Anything you did not read in a tool result this turn does not exist.

Compose every sentence of your answer by quoting a sentence from a tool result verbatim, and \
name the tool call you took it from and the document ids that call cited for it. A sentence you \
wrote yourself, or copied from the question, will be discarded before it is spoken.

Convert money to minor units before calling verify_claim. The server takes an integer count of \
minor units and will not read any other form: one dirham is 100 fils, so AED 1,470,000.00 is \
147000000. A value in the wrong form comes back UNVERIFIED, which is a refusal to guess, not a \
finding about the project. If the person was vague - 'about one and a half million' - do not \
call the tool at all; ask which figure they mean.

A fact whose citation did not verify is spoken as unverified, and the underlying claim is not \
stated. A refusal is an answer: say the caller is not authorised and stop. Never call another \
tool to obtain a fact that was just withheld, and never report a refusal as a finding about the \
project.";

/// One tool as `tools/list` advertised it.
#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    /// The name to call it by.
    pub name: String,
    /// What the server says it is for.
    pub description: String,
    /// The JSON Schema for its arguments.
    pub input_schema: Value,
}

/// One tool call the model wants made.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolUse {
    /// The tool name, which must be one the server advertised.
    pub tool: String,
    /// The arguments, exactly as they will be sent.
    pub arguments: Map<String, Value>,
}

impl ToolUse {
    /// A call with no arguments.
    pub fn bare(tool: &str) -> Self {
        Self {
            tool: tool.to_owned(),
            arguments: Map::new(),
        }
    }

    /// A call with string arguments, in the order given.
    pub fn with(tool: &str, arguments: &[(&str, &str)]) -> Self {
        Self {
            tool: tool.to_owned(),
            arguments: arguments
                .iter()
                .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
                .collect(),
        }
    }
}

/// What one tool call returned, as the model sees it.
#[derive(Clone, Debug, Serialize)]
pub struct ToolOutcome {
    /// Which call this was, counting from zero across the whole turn.
    pub index: usize,
    /// The tool that was called.
    pub tool: String,
    /// The arguments that were sent.
    pub arguments: Value,
    /// The structured result the server returned.
    pub result: Value,
    /// Whether the server marked the result an error — a refusal, or a
    /// request this client got wrong.
    pub refused: bool,
}

/// How far a statement may be taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// The server stated it and cited a document that verified in that call.
    Verified,
    /// The server read the field and it is **not** what was claimed. A
    /// finding either way, and never to be heard as the other one.
    Contradicted,
    /// The server would not establish it. It is repeated as unverified, and
    /// the underlying claim is not stated.
    Unverified,
    /// The server refused the call. The refusal is the answer.
    Refused,
}

/// One sentence of a draft answer, bound to where it came from.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Statement {
    /// Index into the outcomes the model was given: the call this came from.
    pub from_call: usize,
    /// The sentence, copied verbatim out of that call's result.
    pub quoted: String,
    /// The document ids that call cited for it.
    pub citations: Vec<String>,
    /// How far it may be taken.
    pub standing: Standing,
}

/// An answer, before the loop has checked it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct Draft {
    /// The sentences to speak, in order.
    pub statements: Vec<Statement>,
}

/// One step of the tool-use loop.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Run these tools and come back with their results. More than one tool
    /// in a step is one turn of a compound question, not one turn each.
    UseTools(Vec<ToolUse>),
    /// Compose this answer and stop.
    Answer(Draft),
    /// Ask the person something before any tool is called. Only valid before
    /// the first call: once a tool has answered, the turn owes an answer, and
    /// a question back would be a way to avoid stating a refusal.
    Clarify {
        /// What to ask. It states nothing about the project.
        question: String,
    },
}

/// Why a model could not produce a step.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// The model needs a tool the server did not advertise.
    #[error("this loop needs the `{tool}` tool, which the server did not advertise")]
    ToolNotAdvertised {
        /// The tool that is missing.
        tool: String,
    },
    /// The model was asked for a step it has no rule for.
    #[error("no rule for this question after {rounds} round(s) of tool results")]
    NoRule {
        /// How many rounds had already run.
        rounds: usize,
    },
    /// A hosted model could not be reached, or would not answer.
    ///
    /// The detail is a fixed phrase, never an SDK error: a service error's
    /// `Display` can carry the request URL and its query parameters.
    #[error("the hosted model did not answer: {detail}")]
    Unavailable {
        /// What happened, in words chosen here rather than by the SDK.
        detail: String,
    },
}

/// The future a [`Model`] returns.
///
/// Mirrors [`Transcribing`](crate::transcribe::Transcribing), for the same
/// reason: `async fn` in a trait is not dyn-compatible, and this crate holds
/// its implementations behind `Box<dyn …>`. A stub decides in nanoseconds and
/// a hosted model takes a network round trip; both have to satisfy one trait,
/// and making the fast one await costs nothing next to making the slow one
/// block a runtime thread.
pub type Stepping<'a> = Pin<Box<dyn Future<Output = Result<Step, ModelError>> + Send + 'a>>;

/// A model that chooses tool calls and composes an answer from their results.
pub trait Model: Send + Sync {
    /// What to call this model in the page.
    ///
    /// Shown beside the voice's name, so a viewer can see which
    /// implementation produced the answer in front of them rather than
    /// assuming the hosted one.
    fn name(&self) -> &str;

    /// Whether the question and the tool results leave this machine.
    ///
    /// Required rather than defaulted: a default of `false` would mean the
    /// implementation that forgets to answer is the one that claims nothing
    /// is sent anywhere.
    fn sends_questions_off_this_machine(&self) -> bool;

    /// The rules this model works under, shown in the page's debug drawer.
    fn system_prompt(&self) -> &str;

    /// Choose the next step, given everything this turn has seen so far.
    ///
    /// `seen` is empty on the first round and grows by one entry per tool call
    /// made. `tools` is what `tools/list` advertised, which is the only set of
    /// tools a step may name.
    fn next_step<'a>(
        &'a self,
        question: &'a str,
        tools: &'a [ToolSpec],
        seen: &'a [ToolOutcome],
    ) -> Stepping<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_prompt_carries_the_minor_unit_rule() {
        assert!(
            SYSTEM_PROMPT.contains("147000000"),
            "the worked conversion is the thing most likely to go wrong on camera",
        );
        assert!(SYSTEM_PROMPT.contains("100 fils"));
    }

    #[test]
    fn the_system_prompt_carries_the_refusal_rule() {
        assert!(SYSTEM_PROMPT.contains("A refusal is an answer"));
        assert!(SYSTEM_PROMPT.contains("Never call another tool"));
    }

    #[test]
    fn a_tool_use_carries_its_arguments_in_order() {
        let call = ToolUse::with("get_payment_status", &[("unit", "1204")]);
        assert_eq!(call.tool, "get_payment_status");
        assert_eq!(
            call.arguments.get("unit").and_then(Value::as_str),
            Some("1204"),
        );
    }
}
