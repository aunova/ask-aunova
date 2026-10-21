//! The model this build runs on: a deterministic table, not a hosted model.
//!
//! [`StubModel`] maps a handful of scripted questions onto tool calls, and
//! then composes the answer by **quoting the tool results back**. It invents
//! nothing, because it cannot: it has no sentence of its own anywhere in it
//! except the two questions it asks back, and neither of those states anything
//! about the project.
//!
//! Three things it does that a prompt alone would not guarantee:
//!
//! 1. **It converts money to minor units before `verify_claim`**, through
//!    [`crate::money`], and refuses to call the tool at all when the figure is
//!    approximate. A non-canonical value comes back `UNVERIFIED`
//!    `assertion_not_normalised`, which is the server declining to guess — on
//!    a demo that reads as a broken product rather than a careful one.
//! 2. **It sends only argument names the tool declares.** Every schema on that
//!    server is closed, and an undeclared name is a JSON-RPC `-32602` error
//!    rather than a dropped field.
//! 3. **It quotes every statement a result carried, and chooses between
//!    none of them.** Picking the interesting three is a judgement, and a
//!    lookup table that made it would be hiding facts the server was willing
//!    to state. The page lists them with their citations.
//!
//! A question outside the table is a [`ModelError::NoRule`], said plainly. The
//! stub does not improvise, and a stub that improvised would be the one thing
//! this whole project argues against.

use std::ops::Range;

use serde::Serialize;
use serde_json::Value;

use crate::{
    model::{
        Draft, Model, ModelError, SYSTEM_PROMPT, Standing, Statement, Step, Stepping, ToolOutcome,
        ToolSpec, ToolUse,
    },
    money,
};

/// The tool that checks one stated value against one field of the record.
const VERIFY_CLAIM: &str = "verify_claim";

/// Longest run of digits read as a unit number, so a caller's own text is
/// bounded before any of it is sent.
const MAX_UNIT_DIGITS: usize = 8;

/// One question this stub can answer, and what answering it demonstrates.
///
/// The page offers these as buttons. They live here rather than in the page so
/// that the list the page shows and the list the model can actually answer
/// cannot drift apart.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ScriptedQuestion {
    /// The question, exactly as it must be asked for the table to match it.
    pub question: &'static str,
    /// What it is here to show.
    pub shows: &'static str,
}

/// Every question the stub is scripted for.
pub const SCRIPTED_QUESTIONS: [ScriptedQuestion; 7] = [
    ScriptedQuestion {
        question: "Is the project on track, and is the structure certified?",
        shows: "a compound question: two tools/call in one turn, one answer assembled from both",
    },
    ScriptedQuestion {
        question: "What is the payment status of unit 1204?",
        shows: "the unit the buyer token is bound to",
    },
    ScriptedQuestion {
        question: "What is the payment status of unit 0802?",
        shows: "a server-side refusal as a buyer; answered as the developer. Same client, same \
                request, no role in it",
    },
    ScriptedQuestion {
        question: "My statement says unit 1204 has paid 1,470,000.00 dirhams - is that right?",
        shows: "money converted to an exact count of minor units before verify_claim",
    },
    ScriptedQuestion {
        question: "My statement says unit 1204 has paid about one and a half million dirhams - \
                   is that right?",
        shows: "an approximate figure: no tool is called at all, and the question comes back",
    },
    ScriptedQuestion {
        question: "My statement says unit 1204 has paid 1,500,000.00 dirhams - is that right?",
        shows: "a figure the record does not hold: the answer says so plainly and states what \
                the record does say, in dirhams",
    },
    ScriptedQuestion {
        question: "What has changed since last time?",
        shows: "the per-caller cursor the server hands back",
    },
];

/// What the stub decided to do with a question before any tool has answered.
enum Plan {
    /// Make these calls, in this order, as one turn.
    Calls(Vec<ToolUse>),
    /// Ask this back instead. It states nothing about the project.
    Clarify(String),
}

/// A deterministic model: the same question always produces the same calls.
#[derive(Clone, Copy, Debug, Default)]
pub struct StubModel;

impl Model for StubModel {
    fn name(&self) -> &str {
        "scripted stub: a lookup table, no model and no network call"
    }

    fn sends_questions_off_this_machine(&self) -> bool {
        false
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    fn next_step<'a>(
        &'a self,
        question: &'a str,
        tools: &'a [ToolSpec],
        seen: &'a [ToolOutcome],
    ) -> Stepping<'a> {
        // This stub decides from a lookup table and never awaits anything;
        // the future is ready on first poll. The trait is a future because a
        // hosted model needs one, not because this one does.
        Box::pin(async move { Self::decide(question, tools, seen) })
    }
}

impl StubModel {
    /// The whole of the stub's decision, kept synchronous so the table it
    /// reads stays readable as a table.
    fn decide(
        question: &str,
        tools: &[ToolSpec],
        seen: &[ToolOutcome],
    ) -> Result<Step, ModelError> {
        if !seen.is_empty() {
            return Ok(Step::Answer(compose(seen)));
        }
        match plan(question).ok_or(ModelError::NoRule { rounds: 0 })? {
            Plan::Clarify(question) => Ok(Step::Clarify { question }),
            Plan::Calls(calls) => {
                for call in &calls {
                    if !tools.iter().any(|tool| tool.name == call.tool) {
                        return Err(ModelError::ToolNotAdvertised {
                            tool: call.tool.clone(),
                        });
                    }
                }
                Ok(Step::UseTools(calls))
            }
        }
    }
}

/// Resolve a question to the calls that answer it, or to a question back.
///
/// The order of these rules is the whole of the stub's "reasoning": a question
/// that repeats a figure is a claim to check before it is anything else.
fn plan(question: &str) -> Option<Plan> {
    if mentions(question, &CHECK_PHRASES) {
        return Some(claim_check(question));
    }

    let progress = mentions(question, &PROGRESS_WORDS);
    let certificates = mentions(question, &CERTIFICATE_WORDS);
    if progress && certificates {
        return Some(Plan::Calls(vec![
            ToolUse::bare("get_milestones"),
            ToolUse::bare("get_certificates"),
        ]));
    }
    if mentions(question, &PAYMENT_WORDS) {
        return Some(match unit_in(question) {
            // `get_payment_status` is the record itself and states no
            // sentences; `ask_project` states the same schedule as facts that
            // can be quoted. Both are sent so the drawer shows the raw
            // schedule beside the words spoken from it — and a unit outside
            // the caller's scope is refused by both, which is the point.
            Some((unit, _span)) => Plan::Calls(vec![
                ToolUse::with("get_payment_status", &[("unit", &unit)]),
                ToolUse::with("ask_project", &[("topic", "payments"), ("unit", &unit)]),
            ]),
            // The client does not know who it is authenticated as and must not
            // guess: the token decides that, server-side.
            None => Plan::Clarify(
                "Which unit should I check? Give me the unit number and I will ask the server \
                 for that one."
                    .to_owned(),
            ),
        });
    }
    if mentions(question, &CHANGE_WORDS) {
        return Some(Plan::Calls(vec![ToolUse::bare("whats_changed")]));
    }
    if progress {
        return Some(Plan::Calls(vec![ToolUse::bare("get_milestones")]));
    }
    if certificates {
        return Some(Plan::Calls(vec![ToolUse::bare("get_certificates")]));
    }
    None
}

/// A repeated figure, resolved into one typed assertion — or refused.
fn claim_check(question: &str) -> Plan {
    let Some((unit, span)) = unit_in(question) else {
        return Plan::Clarify(
            "Which unit is that statement for? Give me the unit number and I will put that \
             figure to the server."
                .to_owned(),
        );
    };
    // The unit number is a figure too. It is removed before the amount is
    // read, so `unit 1204 ... 1,470,000.00` is one amount and not two.
    let amount = format!("{} {}", &question[..span.start], &question[span.end..]);
    match money::to_minor_units(&amount) {
        Ok(minor) => Plan::Calls(vec![ToolUse::with(
            VERIFY_CLAIM,
            &[
                ("subject_type", "unit"),
                ("subject_id", &unit),
                ("field", "paid_to_date_minor"),
                ("asserted_value", &minor),
            ],
        )]),
        // Not a failure to parse so much as a refusal to invent: a near miss
        // comes back CONTRADICTED, which is a true answer to a question
        // nobody asked.
        Err(reason) => Plan::Clarify(format!(
            "{reason}. Tell me the exact figure and I will put that to the server.",
        )),
    }
}

/// Words that mean the person is repeating a claim for checking.
const CHECK_PHRASES: [&str; 5] = [
    "is that right",
    "is that correct",
    "does the record",
    "statement says",
    "am i right",
];

/// Words that mean the question is about how far along the project is.
const PROGRESS_WORDS: [&str; 5] = [
    "on track",
    "on schedule",
    "progress",
    "milestone",
    "how far",
];

/// Words that mean the question is about compliance documents.
const CERTIFICATE_WORDS: [&str; 4] = ["certif", "signed off", "sign-off", "compliance"];

/// Words that mean the question is about money owed or paid.
const PAYMENT_WORDS: [&str; 6] = [
    "payment",
    "paid",
    "instalment",
    "installment",
    "escrow",
    "overdue",
];

/// Words that mean the question is "what is new".
const CHANGE_WORDS: [&str; 4] = ["changed", "anything new", "what's new", "since last time"];

/// Whether any of `needles` appears in `question`, ignoring ASCII case.
fn mentions(question: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| find_ascii_ci(question, needle, 0).is_some())
}

/// The unit number named in a question, with the byte range that named it.
///
/// Scanned on the original string rather than a lowercased copy: lowercasing
/// can change a string's length, and an offset taken from the copy would then
/// cut the original in the wrong place.
fn unit_in(question: &str) -> Option<(String, Range<usize>)> {
    let bytes = question.as_bytes();
    let mut from = 0usize;
    while let Some(found) = find_ascii_ci(question, "unit", from) {
        let after = found + "unit".len();
        let mut cursor = after;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let digits = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor > digits && cursor - digits <= MAX_UNIT_DIGITS {
            return Some((question[digits..cursor].to_owned(), found..cursor));
        }
        from = after;
    }
    None
}

/// Where `needle` next appears in `haystack`, ignoring ASCII case.
///
/// `needle` is ASCII, and an ASCII byte never occurs inside a multi-byte UTF-8
/// sequence, so a byte-wise match is always on a character boundary.
fn find_ascii_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let hay = haystack.as_bytes();
    let pattern = needle.as_bytes();
    if pattern.is_empty() || hay.len() < pattern.len() {
        return None;
    }
    (from..=hay.len() - pattern.len())
        .find(|&start| hay[start..start + pattern.len()].eq_ignore_ascii_case(pattern))
}

/// Build the answer by quoting what came back, and nothing else.
fn compose(seen: &[ToolOutcome]) -> Draft {
    let mut statements: Vec<Statement> = Vec::new();
    for outcome in seen {
        for statement in statements_from(outcome) {
            // Two calls in one turn can carry the same sentence — a refused
            // unit refuses both tools with one message. Saying it twice adds
            // nothing, and the match is on the whole statement, so no citation
            // is ever dropped by this.
            let duplicate = statements.iter().any(|kept| {
                kept.quoted == statement.quoted
                    && kept.citations == statement.citations
                    && kept.standing == statement.standing
            });
            if !duplicate {
                statements.push(statement);
            }
        }
    }
    Draft { statements }
}

/// The statements one tool result supports.
fn statements_from(outcome: &ToolOutcome) -> Vec<Statement> {
    if outcome.refused {
        return refusal(outcome).into_iter().collect();
    }
    if outcome.tool == VERIFY_CLAIM {
        return verdict(outcome).into_iter().collect();
    }
    let mut facts = Vec::new();
    collect_facts(&outcome.result, outcome.index, &mut facts);
    facts
}

/// A refusal, quoted as the refusal it is.
fn refusal(outcome: &ToolOutcome) -> Option<Statement> {
    let message = outcome
        .result
        .get("message")
        .or_else(|| outcome.result.get("answer"))
        .and_then(Value::as_str)?;
    Some(Statement {
        from_call: outcome.index,
        quoted: message.to_owned(),
        citations: Vec::new(),
        standing: Standing::Refused,
    })
}

/// What `verify_claim` concluded, read as the three verdicts it has.
///
/// `SUPPORTED` and `CONTRADICTED` are both comparisons the server actually
/// made against a named field, so the finding is a fact about the record
/// either way — but they are **not** the same finding, and the server states
/// them in the same sentence: "The signed record states `paid_to_date_minor`
/// for unit 1204 as `147000000`." is the whole of it whether the claim
/// matched or not. Which one it was is in the `verdict`, so that is what is
/// carried here, and [`crate::turn`] says the difference out loud. A verdict
/// a person cannot tell apart from agreement is worth nothing.
///
/// `UNVERIFIED` is a refusal to judge, and is spoken as one.
fn verdict(outcome: &ToolOutcome) -> Option<Statement> {
    let finding = outcome.result.get("finding").and_then(Value::as_str)?;
    let standing = match outcome.result.get("verdict").and_then(Value::as_str) {
        Some("SUPPORTED") => Standing::Verified,
        Some("CONTRADICTED") => Standing::Contradicted,
        _ => Standing::Unverified,
    };
    let citations = outcome
        .result
        .get("supporting_documents")
        .and_then(Value::as_array)
        .map(|documents| {
            documents
                .iter()
                .filter_map(|document| document.get("doc_id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(Statement {
        from_call: outcome.index,
        quoted: finding.to_owned(),
        citations,
        standing,
    })
}

/// Every `{statement, citations}` pair in a result, in the order it appears.
///
/// The server composes those pairs itself — milestones, certificates,
/// `ask_project` facts and `whats_changed` changes all carry them — so this
/// needs no table of tool names and cannot fall behind one.
fn collect_facts(value: &Value, index: usize, into: &mut Vec<Statement>) {
    match value {
        Value::Object(fields) => {
            if let Some(statement) = fields.get("statement").and_then(Value::as_str) {
                into.push(Statement {
                    from_call: index,
                    quoted: statement.to_owned(),
                    citations: fields
                        .get("citations")
                        .and_then(Value::as_array)
                        .map(|ids| {
                            ids.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default(),
                    // The server's own flag, not this client's reading of the
                    // sentence: a certificate whose document did not verify
                    // in this call is said as unverified.
                    standing: match fields.get("may_be_relied_on") {
                        Some(Value::Bool(false)) => Standing::Unverified,
                        _ => Standing::Verified,
                    },
                });
            }
            for field in fields.values() {
                collect_facts(field, index, into);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_facts(item, index, into);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The argument names each tool declares, copied from the server's
    /// schemas. A name outside these is a JSON-RPC `-32602`, not a dropped
    /// field, so the stub is held to them here and again over real HTTP in
    /// `tests/acceptance.rs`.
    const DECLARED: [(&str, &[&str]); 6] = [
        ("get_milestones", &[]),
        ("get_certificates", &[]),
        ("get_payment_status", &["unit"]),
        (
            "verify_claim",
            &[
                "subject_type",
                "subject_id",
                "field",
                "asserted_value",
                "instalment_id",
            ],
        ),
        ("ask_project", &["topic", "unit"]),
        ("whats_changed", &["since_cursor"]),
    ];

    fn calls(question: &str) -> Vec<ToolUse> {
        match plan(question) {
            Some(Plan::Calls(calls)) => calls,
            Some(Plan::Clarify(back)) => panic!("`{question}` asked back instead: {back}"),
            None => panic!("`{question}` matched no rule"),
        }
    }

    fn outcome(tool: &str, result: Value, refused: bool) -> ToolOutcome {
        ToolOutcome {
            index: 0,
            tool: tool.to_owned(),
            arguments: json!({}),
            result,
            refused,
        }
    }

    #[test]
    fn a_compound_question_makes_two_calls_in_one_turn() {
        let calls = calls(SCRIPTED_QUESTIONS[0].question);
        assert_eq!(
            calls
                .iter()
                .map(|call| call.tool.as_str())
                .collect::<Vec<_>>(),
            vec!["get_milestones", "get_certificates"],
        );
    }

    #[test]
    fn a_repeated_amount_is_converted_to_minor_units_before_the_call() {
        let calls = calls(SCRIPTED_QUESTIONS[3].question);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, VERIFY_CLAIM);
        assert_eq!(
            calls[0]
                .arguments
                .get("asserted_value")
                .and_then(Value::as_str),
            Some("147000000"),
            "1,470,000.00 dirhams is 147000000 fils; anything else is UNVERIFIED",
        );
        assert_eq!(
            calls[0].arguments.get("subject_id").and_then(Value::as_str),
            Some("1204"),
            "the unit number must not be read as part of the amount",
        );
    }

    #[test]
    fn an_approximate_amount_calls_no_tool_at_all() {
        match plan(SCRIPTED_QUESTIONS[4].question) {
            Some(Plan::Clarify(back)) => {
                assert!(back.contains("exact figure"), "{back}");
                assert!(
                    !back.contains("1,470,000"),
                    "a question back must not state a figure nobody said: {back}",
                );
            }
            other => panic!(
                "an approximate figure must not reach the server: {}",
                match other {
                    Some(Plan::Calls(calls)) => format!("{calls:?}"),
                    _ => "no rule".to_owned(),
                },
            ),
        }
    }

    #[test]
    fn every_scripted_question_resolves_to_something() {
        for scripted in SCRIPTED_QUESTIONS {
            assert!(
                plan(scripted.question).is_some(),
                "the page offers `{}`, which the model cannot answer",
                scripted.question,
            );
        }
    }

    #[test]
    fn no_plan_ever_sends_an_argument_name_the_tool_does_not_declare() {
        for scripted in SCRIPTED_QUESTIONS {
            let Some(Plan::Calls(calls)) = plan(scripted.question) else {
                continue;
            };
            for call in calls {
                let (_, declared) = DECLARED
                    .iter()
                    .find(|(name, _)| *name == call.tool)
                    .unwrap_or_else(|| panic!("`{}` is not a declared tool", call.tool));
                for sent in call.arguments.keys() {
                    assert!(
                        declared.contains(&sent.as_str()),
                        "`{}` does not declare `{sent}`; that is a -32602, not a dropped field",
                        call.tool,
                    );
                }
            }
        }
    }

    #[test]
    fn a_question_outside_the_table_is_said_to_be_outside_the_table() {
        assert!(plan("what is the weather in the lobby").is_none());
    }

    #[test]
    fn a_refusal_is_quoted_as_a_refusal() {
        let refused = outcome(
            "get_payment_status",
            json!({
                "status": "refused",
                "reason": "role_scope",
                "message": "This bearer token is bound to unit 1204.",
            }),
            true,
        );
        let statements = statements_from(&refused);
        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0].standing, Standing::Refused);
        assert_eq!(
            statements[0].quoted,
            "This bearer token is bound to unit 1204."
        );
    }

    #[test]
    fn a_certificate_that_did_not_verify_is_quoted_as_unverified() {
        let answered = outcome(
            "get_certificates",
            json!({
                "certificates": [
                    {
                        "statement": "The structural certificate verified in this call.",
                        "citations": ["CERT-STRUCT-2026-114"],
                        "may_be_relied_on": true,
                    },
                    {
                        "statement": "The record records CERT-MEP-2026-052 as issued, but its \
                                      document did not verify in this call.",
                        "citations": ["PROJECT-RECORD"],
                        "may_be_relied_on": false,
                    },
                ],
            }),
            false,
        );
        let statements = statements_from(&answered);
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0].standing, Standing::Verified);
        assert_eq!(statements[1].standing, Standing::Unverified);
    }

    #[test]
    fn a_verdict_carries_the_documents_the_server_compared_against() {
        let judged = outcome(
            VERIFY_CLAIM,
            json!({
                "verdict": "SUPPORTED",
                "finding": "The record puts paid_to_date_minor for unit 1204 at 147000000.",
                "supporting_documents": [{"doc_id": "PS-1204"}, {"doc_id": "PROJECT-RECORD"}],
            }),
            false,
        );
        let statements = statements_from(&judged);
        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0].standing, Standing::Verified);
        assert_eq!(statements[0].citations, vec!["PS-1204", "PROJECT-RECORD"]);
    }

    #[test]
    fn an_unverified_verdict_is_never_spoken_as_a_finding() {
        let judged = outcome(
            VERIFY_CLAIM,
            json!({
                "verdict": "UNVERIFIED",
                "reason": "assertion_not_normalised",
                "finding": "The value stated is not in the form this field is read in.",
                "supporting_documents": [],
            }),
            false,
        );
        assert_eq!(statements_from(&judged)[0].standing, Standing::Unverified);
    }

    #[test]
    fn the_same_sentence_from_two_refused_calls_is_said_once() {
        let message = json!({"status": "refused", "message": "Bound to unit 1204."});
        let draft = compose(&[
            outcome("get_payment_status", message.clone(), true),
            ToolOutcome {
                index: 1,
                ..outcome("ask_project", message, true)
            },
        ]);
        assert_eq!(draft.statements.len(), 1);
    }

    #[test]
    fn a_unit_is_read_from_the_question_and_nowhere_else() {
        assert_eq!(
            unit_in("payment status of unit 0802?").map(|(unit, _)| unit),
            Some("0802".to_owned()),
        );
        assert_eq!(unit_in("am I behind on payments?"), None);
        assert_eq!(unit_in("the units 1204 and 1510"), None);
    }

    #[test]
    fn a_unit_offset_survives_text_that_is_not_ascii() {
        // A lowercased copy can change length; an offset taken from one and
        // used on the other cuts in the wrong place, or panics.
        let question = "Grüße — my statement says unit 1204 has paid 1,470,000.00 dirhams";
        let (unit, span) = unit_in(question).expect("the unit is named");
        assert_eq!(unit, "1204");
        assert_eq!(&question[span], "unit 1204");
    }

    #[test]
    fn a_question_with_no_unit_asks_which_unit_rather_than_guessing() {
        match plan("am I behind on payments?") {
            Some(Plan::Clarify(back)) => assert!(back.contains("Which unit"), "{back}"),
            _ => panic!("the client must never pick a unit the person did not name"),
        }
    }
}
