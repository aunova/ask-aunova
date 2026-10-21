//! The loop, and the rules it enforces.
//!
//! This is the product. A model chooses tool calls; the server answers them;
//! and then *this* module decides what may be said, because a model cannot be
//! trusted to mark its own homework.
//!
//! Four rules are enforced here rather than asked for in a prompt:
//!
//! 1. **Every sentence is quoted from a tool result.** A statement names the
//!    call it came from, and the sentence must be present, verbatim, as a
//!    string value in that call's recorded result. A sentence the model wrote,
//!    or copied out of the question, matches nothing and is withheld. So is a
//!    document id the call did not cite.
//! 2. **A refusal is an answer.** Once a call has come back refused, the loop
//!    will not issue another tool call in that turn. There is no way to ask a
//!    second tool for a fact the first one just withheld.
//! 3. **Only advertised tools are called.** The names come from `tools/list`,
//!    so a model inventing a tool is stopped before a request is sent.
//! 4. **A question back comes before the first call, or not at all.** Once a
//!    tool has answered, the turn owes an answer; a late clarification would
//!    be a way to avoid stating a refusal.
//!
//! Everything a statement is spoken with, apart from the quoted sentence, is
//! structural: "I can't verify this." states nothing about the project, and
//! exists because the skill requires an unverified fact to be spoken as
//! unverified.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::{
    mcp::{McpError, McpSession, ServerIdentity},
    model::{Model, ModelError, Standing, Statement, Step, ToolOutcome, ToolSpec},
    money,
    voice::{Speech, Voice},
};

/// How many times the model may be asked for a step in one turn.
pub const MAX_ROUNDS: usize = 4;

/// How many tool calls one turn may make in total.
pub const MAX_CALLS: usize = 6;

/// Said before an unverified statement, so it is never heard as a fact.
const UNVERIFIED_PREFIX: &str = "I can't verify this.";

/// Said before a contradicted claim, so a verdict that disagrees with the
/// person can never be heard as one that agrees. The server states both
/// verdicts in the same sentence — it names the field and what the record
/// holds — so without this they are the same answer.
const CONTRADICTED_PREFIX: &str = "That is not what the record says.";

/// Said before a refusal, so it is never heard as a finding.
const REFUSED_PREFIX: &str = "The server refused that.";

/// One tool call, exactly as it was made and answered.
#[derive(Clone, Debug, Serialize)]
pub struct RecordedCall {
    /// Position in the turn, counting from zero.
    pub index: usize,
    /// The tool that was called.
    pub tool: String,
    /// The arguments that were sent, verbatim.
    pub arguments: Value,
    /// The structured result that came back, verbatim.
    pub result: Value,
    /// Whether the server marked the result an error.
    pub refused: bool,
}

/// One sentence of the answer, and where it came from.
#[derive(Clone, Debug, Serialize)]
pub struct SpokenStatement {
    /// The call this was quoted from.
    pub from_call: usize,
    /// That call's tool, repeated so the page need not cross-reference.
    pub tool: String,
    /// The sentence, verbatim from the result: the thing that was checked.
    pub text: String,
    /// The same sentence in the form it is said and shown.
    ///
    /// They differ in exactly one way, and only for money: the record counts
    /// in minor units, so a `verify_claim` finding states `147000000`, and
    /// nobody hears that as one million four hundred seventy thousand
    /// dirhams. [`text`](Self::text) is what the server wrote and what the
    /// grounding check was run against; this is how it is read to a person.
    pub said: String,
    /// The document ids that call cited for it.
    pub citations: Vec<String>,
    /// How far it may be taken.
    pub standing: Standing,
}

/// A statement the loop refused to speak.
#[derive(Clone, Debug, Serialize)]
pub struct Withheld {
    /// The call it claimed to come from, when that call exists.
    pub from_call: Option<usize>,
    /// What the model wanted said.
    pub text: String,
    /// Why it was not said.
    pub reason: String,
}

/// What the person is told.
#[derive(Clone, Debug, Serialize)]
pub struct Answer {
    /// The whole answer as one piece of text, ready to be spoken.
    pub spoken: String,
    /// Each sentence, with its provenance.
    pub statements: Vec<SpokenStatement>,
    /// Every document id cited, deduplicated.
    pub citations: Vec<String>,
    /// Statements the loop would not speak, and why.
    pub withheld: Vec<Withheld>,
    /// True when this is a question back to the person rather than an answer.
    pub is_clarification: bool,
}

/// One complete exchange: the question, every call it made, and the answer.
#[derive(Clone, Debug, Serialize)]
pub struct Turn {
    /// What was asked.
    pub question: String,
    /// The rules the model worked under, for the page to show.
    pub system_prompt: String,
    /// What the server said it was in the handshake.
    pub server: ServerIdentity,
    /// The tool names `tools/list` advertised.
    pub tools_advertised: Vec<String>,
    /// Every `tools/call` made, in order.
    pub calls: Vec<RecordedCall>,
    /// The answer, after the rules above were applied.
    pub answer: Answer,
    /// What the voice did with it.
    pub speech: Speech,
}

/// Why a turn could not be completed.
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// The MCP exchange failed.
    #[error(transparent)]
    Mcp(#[from] McpError),
    /// The model could not choose a step.
    #[error("the model could not choose a step: {0}")]
    Model(#[from] ModelError),
    /// The model named a tool `tools/list` did not advertise.
    #[error("the model asked for `{tool}`, which this server does not advertise")]
    UndeclaredTool {
        /// The tool it asked for.
        tool: String,
    },
    /// The model tried to keep calling tools after a refusal.
    #[error(
        "the model tried to call `{tool}` after a refusal; a refusal is an answer, not a step to \
         route around"
    )]
    RoutedAroundRefusal {
        /// The tool it tried next.
        tool: String,
    },
    /// The model asked a question back after it had already seen results.
    #[error(
        "the model asked a question back after {calls} tool call(s); a question back belongs \
         before the first call"
    )]
    LateClarification {
        /// How many calls had been made.
        calls: usize,
    },
    /// The model chose a step with no tools in it.
    #[error("the model chose to call tools and then named none")]
    NoToolsChosen,
    /// The model never composed an answer.
    #[error("the model did not reach an answer within {MAX_ROUNDS} rounds")]
    NoAnswer,
    /// The model asked for more calls than one turn may make.
    #[error("the model asked for more than {MAX_CALLS} tool calls in one turn")]
    TooManyCalls,
}

/// Run one question through the loop and return everything it did.
pub async fn run_turn(
    session: &McpSession,
    model: &dyn Model,
    voice: &dyn Voice,
    question: &str,
) -> Result<Turn, TurnError> {
    let tools = session.list_tools().await?;
    let mut calls: Vec<RecordedCall> = Vec::new();
    let mut seen: Vec<ToolOutcome> = Vec::new();
    let mut answer: Option<Answer> = None;

    for _round in 0..MAX_ROUNDS {
        match model.next_step(question, &tools, &seen).await? {
            Step::Clarify { question } => {
                if !calls.is_empty() {
                    return Err(TurnError::LateClarification { calls: calls.len() });
                }
                answer = Some(clarification(question));
                break;
            }
            Step::Answer(draft) => {
                answer = Some(compose(&draft.statements, &calls));
                break;
            }
            Step::UseTools(uses) => {
                if uses.is_empty() {
                    return Err(TurnError::NoToolsChosen);
                }
                if let Some(refused) = calls.iter().find(|call| call.refused) {
                    tracing::warn!(
                        after = refused.tool,
                        next = uses[0].tool,
                        "refused to route around a refusal",
                    );
                    return Err(TurnError::RoutedAroundRefusal {
                        tool: uses[0].tool.clone(),
                    });
                }
                if calls.len() + uses.len() > MAX_CALLS {
                    return Err(TurnError::TooManyCalls);
                }
                for use_ in uses {
                    if !advertised(&tools, &use_.tool) {
                        return Err(TurnError::UndeclaredTool { tool: use_.tool });
                    }
                    let index = calls.len();
                    let arguments = Value::Object(use_.arguments.clone());
                    let answered = session.call_tool(&use_.tool, use_.arguments).await?;
                    tracing::info!(
                        tool = use_.tool,
                        index,
                        refused = answered.refused,
                        "called a tool",
                    );
                    calls.push(RecordedCall {
                        index,
                        tool: use_.tool.clone(),
                        arguments: arguments.clone(),
                        result: answered.result.clone(),
                        refused: answered.refused,
                    });
                    seen.push(ToolOutcome {
                        index,
                        tool: use_.tool,
                        arguments,
                        result: answered.result,
                        refused: answered.refused,
                    });
                }
            }
        }
    }

    let answer = answer.ok_or(TurnError::NoAnswer)?;
    let speech = voice.speak(&answer.spoken).await;
    Ok(Turn {
        question: question.to_owned(),
        system_prompt: model.system_prompt().to_owned(),
        server: session.server().clone(),
        tools_advertised: tools.iter().map(|tool| tool.name.clone()).collect(),
        calls,
        answer,
        speech,
    })
}

/// Whether `tools/list` advertised a tool by this name.
fn advertised(tools: &[ToolSpec], name: &str) -> bool {
    tools.iter().any(|tool| tool.name == name)
}

/// A question back to the person, stating nothing about the project.
fn clarification(question: String) -> Answer {
    Answer {
        spoken: question,
        statements: Vec::new(),
        citations: Vec::new(),
        withheld: Vec::new(),
        is_clarification: true,
    }
}

/// Check every drafted statement against the call it claims to come from, and
/// assemble what survives.
fn compose(drafted: &[Statement], calls: &[RecordedCall]) -> Answer {
    let mut statements = Vec::new();
    let mut withheld = Vec::new();
    let mut citations: Vec<String> = Vec::new();

    for statement in drafted {
        let Some(call) = calls.get(statement.from_call) else {
            withheld.push(Withheld {
                from_call: None,
                text: statement.quoted.clone(),
                reason: format!(
                    "it names call {}, and this turn made {}",
                    statement.from_call,
                    calls.len(),
                ),
            });
            continue;
        };
        if let Err(reason) = grounded(statement, call) {
            tracing::warn!(
                tool = call.tool,
                reason,
                "withheld a statement that was not in the tool result",
            );
            withheld.push(Withheld {
                from_call: Some(call.index),
                text: statement.quoted.clone(),
                reason,
            });
            continue;
        }
        for doc_id in &statement.citations {
            if !citations.contains(doc_id) {
                citations.push(doc_id.clone());
            }
        }
        statements.push(SpokenStatement {
            from_call: call.index,
            tool: call.tool.clone(),
            text: statement.quoted.clone(),
            said: said_as_money(&statement.quoted, call),
            citations: statement.citations.clone(),
            standing: statement.standing,
        });
    }

    Answer {
        spoken: speak(&statements),
        statements,
        citations,
        withheld,
        is_clarification: false,
    }
}

/// Whether a statement is really in the result it names.
///
/// The sentence must be a string value somewhere in that result, and so must
/// every document id cited for it. Object *keys* are not searched: a field
/// name is not a fact, and quoting one would be a way to pass this check with
/// nothing said.
fn grounded(statement: &Statement, call: &RecordedCall) -> Result<(), String> {
    if statement.quoted.trim().is_empty() {
        return Err("it is empty".to_owned());
    }
    let mut strings = BTreeSet::new();
    collect_strings(&call.result, &mut strings);

    if !strings.contains(statement.quoted.as_str()) {
        return Err(format!(
            "`{}` did not return that sentence, so it is not this server's to state",
            call.tool,
        ));
    }
    for doc_id in &statement.citations {
        if !strings.contains(doc_id.as_str()) {
            return Err(format!(
                "`{}` did not cite {doc_id} for that sentence",
                call.tool,
            ));
        }
    }
    match statement.standing {
        Standing::Refused if !call.refused => Err(format!("`{}` did not refuse", call.tool)),
        Standing::Refused => Ok(()),
        _ if call.refused => Err(format!(
            "`{}` refused, so nothing from it may be stated as a finding",
            call.tool,
        )),
        Standing::Verified | Standing::Contradicted if statement.citations.is_empty() => Err(
            "it cites no document, so it cannot be stated as a finding about the record".to_owned(),
        ),
        Standing::Verified | Standing::Contradicted | Standing::Unverified => Ok(()),
    }
}

/// Every string *value* in a JSON document, keys excluded.
fn collect_strings<'a>(value: &'a Value, into: &mut BTreeSet<&'a str>) {
    match value {
        Value::String(text) => {
            into.insert(text.as_str());
        }
        Value::Array(items) => {
            for item in items {
                collect_strings(item, into);
            }
        }
        Value::Object(fields) => {
            for field in fields.values() {
                collect_strings(field, into);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// The same sentence, with a figure the record counts in minor units said as
/// money.
///
/// The record holds `147000000` and the server states exactly that, because
/// an integer count of minor units is the only form it will compare. Said to
/// a person, that figure is one million four hundred seventy thousand
/// dirhams, and `147000000` is not a number anybody hears as money.
///
/// This is narrow on purpose, and it is the one place a quoted sentence is
/// not repeated character for character. Two things have to be true before a
/// figure is rewritten:
///
/// 1. the call itself named a field counted in minor units — `field` is a
///    `verify_claim` argument, so this is the client's own request, not a
///    guess about what some number in a sentence means;
/// 2. the figure is a run of digits the server put in backticks, which is
///    how it writes the value it read and nothing else. The unit number in
///    "for unit 1204" is not in backticks and is left alone.
///
/// Anything else is returned unchanged, and the verbatim sentence is kept in
/// [`SpokenStatement::text`] and in the recorded result either way, so what
/// was checked is still there to read.
fn said_as_money(text: &str, call: &RecordedCall) -> String {
    let counted_in_minor_units = call
        .arguments
        .get("field")
        .and_then(Value::as_str)
        .is_some_and(|field| field.ends_with("_minor"));
    if !counted_in_minor_units {
        return text.to_owned();
    }

    let mut said = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            break;
        };
        let quoted = &after[..close];
        let digits = !quoted.is_empty() && quoted.bytes().all(|byte| byte.is_ascii_digit());
        match digits
            .then(|| quoted.parse::<i128>().ok())
            .flatten()
            .and_then(money::in_words)
        {
            Some(words) => {
                said.push_str(&rest[..open]);
                said.push_str(&words);
            }
            // Not a figure, or one too large to say: left exactly as the
            // server wrote it, backticks and all.
            None => said.push_str(&rest[..open + 1 + close + 1]),
        }
        rest = &after[close + 1..];
    }
    said.push_str(rest);
    said
}

/// Assemble the statements into one piece of text to speak.
///
/// Each sentence is the server's own; the prefixes and the bracketed citation
/// list are structure, and state nothing about the project.
fn speak(statements: &[SpokenStatement]) -> String {
    let mut spoken = String::new();
    for statement in statements {
        if !spoken.is_empty() {
            spoken.push(' ');
        }
        match statement.standing {
            Standing::Verified => {}
            Standing::Contradicted => {
                spoken.push_str(CONTRADICTED_PREFIX);
                spoken.push(' ');
            }
            Standing::Unverified => {
                spoken.push_str(UNVERIFIED_PREFIX);
                spoken.push(' ');
            }
            Standing::Refused => {
                spoken.push_str(REFUSED_PREFIX);
                spoken.push(' ');
            }
        }
        spoken.push_str(&statement.said);
        if !statement.citations.is_empty() {
            spoken.push_str(" [");
            spoken.push_str(&statement.citations.join(", "));
            spoken.push(']');
        }
    }
    spoken
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(result: Value, refused: bool) -> RecordedCall {
        RecordedCall {
            index: 0,
            tool: "get_milestones".to_owned(),
            arguments: json!({}),
            result,
            refused,
        }
    }

    /// A `verify_claim` call: the arguments name the field, and the field is
    /// what says the figure in the finding is a count of minor units.
    fn verify_call(verdict: &str) -> RecordedCall {
        RecordedCall {
            index: 0,
            tool: "verify_claim".to_owned(),
            arguments: json!({
                "subject_type": "unit",
                "subject_id": "1204",
                "field": "paid_to_date_minor",
                "asserted_value": "150000000",
            }),
            result: json!({
                "verdict": verdict,
                "finding": "The signed record states `paid_to_date_minor` for unit 1204 as \
                            `147000000`.",
                "supporting_documents": [{"doc_id": "PS-1204"}],
            }),
            refused: false,
        }
    }

    /// The finding, exactly as the server writes it for either verdict.
    fn finding() -> String {
        "The signed record states `paid_to_date_minor` for unit 1204 as `147000000`.".to_owned()
    }

    fn milestones() -> Value {
        json!({
            "milestones": [{
                "id": "MS-07",
                "statement": "Milestone MS-07 is in progress.",
                "citations": ["SR-07", "PROJECT-RECORD"],
            }],
        })
    }

    #[test]
    fn a_quoted_sentence_with_its_citations_is_spoken() {
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: "Milestone MS-07 is in progress.".to_owned(),
                citations: vec!["SR-07".to_owned()],
                standing: Standing::Verified,
            }],
            &[call(milestones(), false)],
        );
        assert_eq!(answer.withheld.len(), 0);
        assert_eq!(answer.spoken, "Milestone MS-07 is in progress. [SR-07]",);
        assert_eq!(answer.citations, vec!["SR-07".to_owned()]);
    }

    #[test]
    fn a_sentence_the_tool_never_returned_is_withheld() {
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: "The tower is finished and fully certified.".to_owned(),
                citations: vec!["SR-07".to_owned()],
                standing: Standing::Verified,
            }],
            &[call(milestones(), false)],
        );
        assert_eq!(answer.statements.len(), 0);
        assert_eq!(answer.spoken, "");
        assert_eq!(answer.withheld.len(), 1);
        assert!(
            answer.withheld[0]
                .reason
                .contains("did not return that sentence"),
            "{}",
            answer.withheld[0].reason,
        );
    }

    #[test]
    fn a_citation_the_tool_never_made_is_withheld() {
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: "Milestone MS-07 is in progress.".to_owned(),
                citations: vec!["SR-99".to_owned()],
                standing: Standing::Verified,
            }],
            &[call(milestones(), false)],
        );
        assert_eq!(answer.statements.len(), 0);
        assert!(answer.withheld[0].reason.contains("did not cite SR-99"));
    }

    #[test]
    fn a_field_name_is_not_a_quotable_fact() {
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: "statement".to_owned(),
                citations: Vec::new(),
                standing: Standing::Unverified,
            }],
            &[call(milestones(), false)],
        );
        assert_eq!(answer.statements.len(), 0);
    }

    #[test]
    fn a_statement_from_a_call_that_never_happened_is_withheld() {
        let answer = compose(
            &[Statement {
                from_call: 4,
                quoted: "Milestone MS-07 is in progress.".to_owned(),
                citations: Vec::new(),
                standing: Standing::Verified,
            }],
            &[call(milestones(), false)],
        );
        assert_eq!(answer.withheld[0].from_call, None);
    }

    #[test]
    fn a_refusal_may_only_be_spoken_as_a_refusal() {
        let refusal = json!({
            "status": "refused",
            "message": "This bearer token is bound to unit 1204.",
        });
        let stated_as_fact = compose(
            &[Statement {
                from_call: 0,
                quoted: "This bearer token is bound to unit 1204.".to_owned(),
                citations: Vec::new(),
                standing: Standing::Verified,
            }],
            &[call(refusal.clone(), true)],
        );
        assert_eq!(stated_as_fact.statements.len(), 0);

        let stated_as_refusal = compose(
            &[Statement {
                from_call: 0,
                quoted: "This bearer token is bound to unit 1204.".to_owned(),
                citations: Vec::new(),
                standing: Standing::Refused,
            }],
            &[call(refusal, true)],
        );
        assert_eq!(stated_as_refusal.statements.len(), 1);
        assert_eq!(
            stated_as_refusal.spoken,
            "The server refused that. This bearer token is bound to unit 1204.",
        );
    }

    #[test]
    fn a_verified_statement_must_cite_something() {
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: "Milestone MS-07 is in progress.".to_owned(),
                citations: Vec::new(),
                standing: Standing::Verified,
            }],
            &[call(milestones(), false)],
        );
        assert_eq!(answer.statements.len(), 0);
        assert!(answer.withheld[0].reason.contains("cites no document"));
    }

    #[test]
    fn an_unverified_statement_is_spoken_as_unverified() {
        let certificates = json!({
            "certificates": [{
                "statement": "The project record records certificate CERT-MEP-2026-052 as \
                              issued, but its stored document did not verify in this call.",
                "citations": ["CERT-MEP-2026-052"],
            }],
        });
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: "The project record records certificate CERT-MEP-2026-052 as issued, but \
                         its stored document did not verify in this call."
                    .to_owned(),
                citations: vec!["CERT-MEP-2026-052".to_owned()],
                standing: Standing::Unverified,
            }],
            &[call(certificates, false)],
        );
        assert!(
            answer.spoken.starts_with(UNVERIFIED_PREFIX),
            "{}",
            answer.spoken,
        );
    }

    #[test]
    fn a_figure_the_record_counts_in_minor_units_is_said_as_dirhams() {
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: finding(),
                citations: vec!["PS-1204".to_owned()],
                standing: Standing::Verified,
            }],
            &[verify_call("SUPPORTED")],
        );
        assert_eq!(answer.withheld.len(), 0, "{answer:?}");
        assert!(
            answer
                .spoken
                .contains("one million four hundred seventy thousand dirhams"),
            "{}",
            answer.spoken,
        );
        assert!(
            !answer.spoken.contains("147000000"),
            "a count of fils is not a figure anybody hears as money: {}",
            answer.spoken,
        );
        // What was checked is still exactly what the server wrote.
        assert_eq!(answer.statements[0].text, finding());
    }

    #[test]
    fn a_contradicted_verdict_is_never_heard_as_an_agreeing_one() {
        // The server states both verdicts in the same words, so this is the
        // only thing standing between "you are right" and "you are wrong".
        let contradicted = compose(
            &[Statement {
                from_call: 0,
                quoted: finding(),
                citations: vec!["PS-1204".to_owned()],
                standing: Standing::Contradicted,
            }],
            &[verify_call("CONTRADICTED")],
        );
        assert_eq!(contradicted.withheld.len(), 0, "{contradicted:?}");
        assert!(
            contradicted.spoken.starts_with(CONTRADICTED_PREFIX),
            "a contradiction has to say so first: {}",
            contradicted.spoken,
        );
        // And it states what the record does say, in the same breath.
        assert!(
            contradicted
                .spoken
                .contains("one million four hundred seventy thousand dirhams"),
            "{}",
            contradicted.spoken,
        );

        let supported = compose(
            &[Statement {
                from_call: 0,
                quoted: finding(),
                citations: vec!["PS-1204".to_owned()],
                standing: Standing::Verified,
            }],
            &[verify_call("SUPPORTED")],
        );
        assert_ne!(
            contradicted.spoken, supported.spoken,
            "a verdict nobody can tell apart from agreement is worth nothing",
        );
    }

    #[test]
    fn a_contradicted_statement_still_has_to_cite_a_document() {
        let answer = compose(
            &[Statement {
                from_call: 0,
                quoted: finding(),
                citations: Vec::new(),
                standing: Standing::Contradicted,
            }],
            &[verify_call("CONTRADICTED")],
        );
        assert_eq!(answer.statements.len(), 0, "{answer:?}");
        assert!(answer.withheld[0].reason.contains("cites no document"));
    }

    #[test]
    fn a_number_that_is_not_money_is_said_exactly_as_the_server_wrote_it() {
        // The unit number is in this sentence too, and "twelve dirhams and
        // four fils" is what reading every number as money would make of it.
        // Only a backticked figure, in a call that named a field counted in
        // minor units, is money.
        let said = said_as_money(&finding(), &verify_call("SUPPORTED"));
        assert!(said.contains("for unit 1204 as"), "{said}");

        // And a call that named no such field is not touched at all.
        let milestone = RecordedCall {
            arguments: json!({
                "subject_type": "milestone",
                "subject_id": "MS-07",
                "field": "percent_complete",
                "asserted_value": "48",
            }),
            ..verify_call("CONTRADICTED")
        };
        let percent = "The signed record states `percent_complete` as `48`.";
        assert_eq!(said_as_money(percent, &milestone), percent);
    }
}
