//! The six tools this server declares, and what each one answers.
//!
//! Every call reaches [`call`], and every call to [`call`] has already been
//! given a [`VerifiedDataset`] produced by a verification run inside that same
//! request. There is no path through this module that answers from an
//! unverified record: when the record itself does not verify the server never
//! gets this far, and [`refusal`] is returned instead.
//!
//! A tool refuses in one of two ways, and the difference matters. A request
//! the caller got wrong — a missing argument, a value outside a closed enum,
//! a cursor this server did not issue — is a JSON-RPC error, because their
//! client should fix it, and because a JSON-RPC error carries no `verdict`
//! field and so cannot be mistaken for one. A request the server will not
//! answer — a buyer asking for another unit, a document that did not verify —
//! is a tool *result* with `isError: true`, so the refusal and its reason
//! reach the person who asked.
//!
//! **No tool takes a sentence.** The arguments below are typed: closed enums,
//! ids, and one stated value under a documented normalisation. Turning a
//! person's words into those arguments is the calling model's job, which is
//! what MCP tool schemas are for; this server's job is to prove the arguments
//! against a named field of a signed record, or to refuse.

use std::{
    collections::BTreeSet,
    sync::{Arc, LazyLock},
};

use rmcp::{
    ErrorData,
    model::{CallToolResult, JsonObject, Tool, ToolAnnotations},
};
use serde_json::Value;

use crate::{
    answers::{self, Topic},
    assertion::{Assertion, Field, SubjectType},
    auth::Role,
    cursor,
    verify::{DatasetUnavailable, VerifiedDataset},
};

/// Every tool's argument schema, built once on first use.
///
/// The schema is the single declaration of what a tool takes: `tools/list`
/// sends this exact object, and [`call`] refuses any argument name that is
/// not one of its `properties`. Building the table once keeps a `tools/list`
/// and a tool call from re-running the same `format!`s.
static SCHEMAS: LazyLock<[Arc<JsonObject>; DeclaredTool::ALL.len()]> =
    LazyLock::new(|| DeclaredTool::ALL.map(DeclaredTool::build_schema));

/// One of the six tools `tools/list` advertises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclaredTool {
    /// Construction milestones and the evidence behind each one.
    GetMilestones,
    /// Certificates, their fictional issuer, and the signature check result.
    GetCertificates,
    /// Instalments and escrow state for one unit.
    GetPaymentStatus,
    /// Check one stated field value against the signed source documents.
    VerifyClaim,
    /// What the record says on one topic, within the caller's scope.
    AskProject,
    /// What changed after the cursor the caller carries.
    WhatsChanged,
}

impl DeclaredTool {
    /// Every declared tool, in the order `tools/list` returns them.
    pub const ALL: [DeclaredTool; 6] = [
        DeclaredTool::GetMilestones,
        DeclaredTool::GetCertificates,
        DeclaredTool::GetPaymentStatus,
        DeclaredTool::VerifyClaim,
        DeclaredTool::AskProject,
        DeclaredTool::WhatsChanged,
    ];

    /// The name a client calls this tool by.
    pub const fn name(self) -> &'static str {
        match self {
            DeclaredTool::GetMilestones => "get_milestones",
            DeclaredTool::GetCertificates => "get_certificates",
            DeclaredTool::GetPaymentStatus => "get_payment_status",
            DeclaredTool::VerifyClaim => "verify_claim",
            DeclaredTool::AskProject => "ask_project",
            DeclaredTool::WhatsChanged => "whats_changed",
        }
    }

    /// Look a tool up by the name a client sent.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.name() == name)
    }

    /// What the tool is for, including how the caller's role changes the answer.
    fn description(self) -> String {
        match self {
            DeclaredTool::GetMilestones => {
                "List the construction milestones of the project with their status, date and the \
                 source documents that evidence each one. Every milestone returned cites at \
                 least one document whose signature was re-checked during this call; one that \
                 cannot be evidenced is listed as withheld instead, with the reason. A buyer \
                 sees the published milestones; the project developer also sees the internal \
                 contractor notes behind them."
                    .to_owned()
            }
            DeclaredTool::GetCertificates => {
                "List the project's certificates with their issuer, issue date and the result of \
                 verifying the issuer's signature over the stored document, recomputed during \
                 this call. A certificate whose document does not verify is still listed, marked \
                 `signature_invalid`, with the failing check named — never as sound. A buyer \
                 sees issued certificates; the project developer also sees pending ones."
                    .to_owned()
            }
            DeclaredTool::GetPaymentStatus => {
                "Report the instalment schedule and escrow state of one unit, in integer minor \
                 units. A buyer may read only the unit their own bearer token is bound to and is \
                 refused any other; the project developer may read any unit. The escrow balance \
                 is stated only when the attestation document behind it verifies in this call."
                    .to_owned()
            }
            DeclaredTool::VerifyClaim => format!(
                "Check one stated value against one named field of the signed record. This tool \
                 does not read sentences: turn what the person said into `subject_type`, \
                 `subject_id`, `field` and `asserted_value` yourself, and send one field per \
                 call. The verdict is SUPPORTED when the record's own value for that field \
                 equals the value you state, CONTRADICTED when it was read and differs, and \
                 UNVERIFIED — with a `reason` — when no comparison happened at all: the subject \
                 is not in the record or not in your scope, the field does not belong to that \
                 subject type, the record states no such value, the value you stated is not one \
                 this server can read, or the document behind the subject did not verify in \
                 this call. A verified document is a precondition for judging, never a verdict. \
                 Values are normalised once, per field type, and the rules are narrow: {}.",
                normalisation_summary(),
            ),
            DeclaredTool::AskProject => {
                "State what the signed record says on one topic. This is a deterministic \
                 composer, not a language model and not a question parser: you choose the \
                 topic, it states every fact the record holds under that topic for your role, \
                 cites the document behind each one, and says \"I cannot verify that\" when \
                 nothing under that topic can be evidenced. Give `unit` only when the person \
                 asked about a particular unit: a buyer naming any unit but their own is \
                 refused, and omitting it means the caller's own units — never a substitute for \
                 a unit that was named."
                    .to_owned()
            }
            DeclaredTool::WhatsChanged => {
                "List what changed in the project record after the point your cursor marks, \
                 measured over the record's own dated events against a seeded baseline. This \
                 server keeps no record of what it has told you: pass back the `next_cursor` \
                 from your previous answer to carry on from it, or omit `since_cursor` to start \
                 from the baseline. Because only a cursor you actually received can be sent \
                 back, a call whose answer never reaches you loses nothing — and a change this \
                 server could not state, because its evidence did not verify, is not in the \
                 cursor it returns, so it is offered again next time. Scoped to what your role \
                 may hear."
                    .to_owned()
            }
        }
    }

    /// The JSON Schema for this tool's arguments, built once per process.
    ///
    /// One object serves both places the declaration is read: `tools/list`
    /// sends it to the client, and [`call`] checks the caller's argument
    /// names against its `properties`. There is exactly one list of declared
    /// names, so the schema's `additionalProperties: false` and the check
    /// that enforces it cannot disagree — a hand-copied second list is how
    /// that drifts back.
    fn schema(self) -> &'static Arc<JsonObject> {
        &SCHEMAS[self.index()]
    }

    /// This tool's position in [`DeclaredTool::ALL`].
    const fn index(self) -> usize {
        match self {
            DeclaredTool::GetMilestones => 0,
            DeclaredTool::GetCertificates => 1,
            DeclaredTool::GetPaymentStatus => 2,
            DeclaredTool::VerifyClaim => 3,
            DeclaredTool::AskProject => 4,
            DeclaredTool::WhatsChanged => 5,
        }
    }

    /// Whether this tool declares an argument by that name.
    ///
    /// Read straight off the schema the client was sent, so the server
    /// accepts exactly the names it advertised and nothing else.
    fn declares(self, name: &str) -> bool {
        self.schema()
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| properties.contains_key(name))
    }

    /// The argument names this tool declares, for an error that says what it
    /// does take.
    fn declared_arguments(self) -> Vec<&'static str> {
        self.schema()
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    fn build_schema(self) -> Arc<JsonObject> {
        match self {
            DeclaredTool::GetMilestones | DeclaredTool::GetCertificates => {
                object_schema(Vec::new(), &[])
            }
            DeclaredTool::GetPaymentStatus => object_schema(
                vec![Property::free(
                    "unit",
                    "Unit number exactly as printed in the sale agreement, for example `1204`.",
                )],
                &["unit"],
            ),
            DeclaredTool::VerifyClaim => verify_claim_schema(),
            DeclaredTool::AskProject => object_schema(
                vec![
                    Property::closed(
                        "topic",
                        "Which part of the record to state. `overview` is the project's own \
                         status and handover date.",
                        Topic::ALL.iter().map(|topic| topic.as_str()).collect(),
                    ),
                    Property::free(
                        "unit",
                        "Optional: the unit the person asked about, exactly as printed in the \
                         sale agreement. Omit it unless a unit was named — omitting it means \
                         the units your own token covers.",
                    ),
                ],
                &["topic"],
            ),
            DeclaredTool::WhatsChanged => object_schema(
                vec![Property::free(
                    "since_cursor",
                    "Optional: the `next_cursor` from your previous `whats_changed` answer. \
                     Omit it to start from the baseline. It is opaque — pass it back unchanged.",
                )],
                &[],
            ),
        }
    }

    /// The full declaration sent in `tools/list`.
    pub fn declaration(self) -> Tool {
        Tool::new(self.name(), self.description(), Arc::clone(self.schema()))
            // Every tool reads the project record; none of them writes to it.
            .annotate(ToolAnnotations::new().read_only(true))
    }
}

/// Every tool declaration, for `tools/list`.
pub fn declared_tools() -> Vec<Tool> {
    DeclaredTool::ALL
        .iter()
        .map(|tool| tool.declaration())
        .collect()
}

/// One declared argument: its name, what it is for, and whether its values are
/// a closed set.
struct Property {
    name: &'static str,
    description: String,
    permitted: Option<Vec<&'static str>>,
}

impl Property {
    /// An argument whose value is a string the server reads.
    fn free(name: &'static str, description: &str) -> Self {
        Self {
            name,
            description: description.to_owned(),
            permitted: None,
        }
    }

    /// An argument whose value must be one of a closed set.
    fn closed(name: &'static str, description: &str, permitted: Vec<&'static str>) -> Self {
        Self {
            name,
            description: format!("{description} One of: {}.", quoted_list(&permitted)),
            permitted: Some(permitted),
        }
    }

    /// This property as a JSON Schema fragment.
    fn schema(&self) -> Value {
        let mut property = JsonObject::new();
        property.insert("type".to_owned(), Value::String("string".to_owned()));
        property.insert(
            "description".to_owned(),
            Value::String(self.description.clone()),
        );
        if let Some(permitted) = &self.permitted {
            property.insert("enum".to_owned(), enum_values(permitted));
        }
        Value::Object(property)
    }
}

/// Build a closed JSON Schema object from declared properties.
fn object_schema(properties: Vec<Property>, required: &[&str]) -> Arc<JsonObject> {
    let mut declared = JsonObject::new();
    for property in &properties {
        declared.insert(property.name.to_owned(), property.schema());
    }

    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), Value::String("object".to_owned()));
    schema.insert("properties".to_owned(), Value::Object(declared));
    if !required.is_empty() {
        schema.insert("required".to_owned(), enum_values(required));
    }
    schema.insert("additionalProperties".to_owned(), Value::Bool(false));
    Arc::new(schema)
}

/// `verify_claim`'s schema: closed enums, plus the dependencies between them.
///
/// `field` is declared as the union of every field name, because JSON Schema's
/// `enum` is one list — and then `allOf` narrows it per `subject_type`, so a
/// client that validates against this schema cannot send a certificate's
/// `paid_to_date_minor` at all. The same `allOf` narrows `asserted_value` to
/// the closed vocabulary of each status field, so a caller can *read* that a
/// milestone's status is spelled `in_progress` instead of learning it from a
/// refusal. Both lists come from [`SubjectType::fields`] and
/// [`FieldKind::vocabulary`], which are the lists the server itself checks
/// against. The server re-checks the pairing regardless and answers
/// UNVERIFIED rather than a verdict, because a schema a client chose not to
/// honour is not a guarantee.
fn verify_claim_schema() -> Arc<JsonObject> {
    let subject_types: Vec<&'static str> = SubjectType::ALL
        .iter()
        .map(|subject_type| subject_type.as_str())
        .collect();
    let mut schema = object_schema(
        vec![
            Property::closed(
                "subject_type",
                "Which part of the record the claim is about.",
                subject_types,
            ),
            Property::free("subject_id", &subject_id_description()),
            Property::closed(
                "field",
                "The one field of that subject the claim states a value for. Which names are \
                 valid depends on `subject_type`.",
                Field::declared_names(),
            ),
            Property::free("asserted_value", &asserted_value_description()),
            Property::free(
                "instalment_id",
                "Required for the per-instalment fields and for nothing else: the instalment \
                 id within that unit's schedule, for example `IN-06`.",
            ),
        ],
        &["subject_type", "subject_id", "field", "asserted_value"],
    );

    // One `allOf` entry per subject type narrows `field`; one more per
    // status field narrows `asserted_value` to that field's own vocabulary.
    let mut narrowed: Vec<Value> = Vec::new();
    for subject_type in SubjectType::ALL {
        let names: Vec<&'static str> = subject_type
            .fields()
            .iter()
            .map(|field| field.as_str())
            .collect();
        narrowed.push(serde_json::json!({
            "if": {"properties": {"subject_type": {"const": subject_type.as_str()}},
                   "required": ["subject_type"]},
            "then": {"properties": {"field": {"enum": enum_values(&names)}}},
        }));

        for field in subject_type.fields() {
            let Some(words) = field.kind().vocabulary() else {
                continue;
            };
            narrowed.push(serde_json::json!({
                "if": {"properties": {"subject_type": {"const": subject_type.as_str()},
                                      "field": {"const": field.as_str()}},
                       "required": ["subject_type", "field"]},
                "then": {"properties": {"asserted_value": {"enum": enum_values(words)}}},
            }));
        }
    }
    Arc::make_mut(&mut schema).insert("allOf".to_owned(), Value::Array(narrowed));
    schema
}

/// What `subject_id` names, per subject type.
fn subject_id_description() -> String {
    let per_type = SubjectType::ALL
        .iter()
        .map(|subject_type| {
            format!(
                "for `{}`, {}",
                subject_type.as_str(),
                subject_type.id_description()
            )
        })
        .collect::<Vec<String>>()
        .join(" ");
    format!("The id of the subject, exactly as the record writes it: {per_type}")
}

/// What `asserted_value` must look like, per field type.
///
/// Each rule shows the canonical form with a worked example, because a
/// calling model reads this string and nothing else: one conversion in the
/// description does more than three sentences of rule to stop a caller
/// sending whole dirhams where the record keeps fils.
fn asserted_value_description() -> String {
    format!(
        "The value the claim states for that field, as a string, written in the field's own \
         canonical form, normalised once per field type and then compared exactly: {}. A \
         value that is not in that form is answered UNVERIFIED with reason \
         `assertion_not_normalised`, never compared loosely and never called false — this \
         server does not guess at what you meant.",
        normalisation_summary(),
    )
}

/// Every normalisation rule, in one sentence per field type.
///
/// Each kind is taken from a field that really has it, so this list cannot
/// describe a rule no field is read under.
fn normalisation_summary() -> String {
    [
        (Field::PaidToDateMinor, "money"),
        (Field::MilestoneActualDate, "dates"),
        (Field::MilestonePercentComplete, "percentages"),
        (Field::HasOverdueInstalment, "flags"),
        (Field::MilestoneStatus, "status fields"),
        (Field::MilestoneTitle, "text fields"),
    ]
    .iter()
    .map(|(field, label)| format!("{label} are {}", field.kind().rule()))
    .collect::<Vec<String>>()
    .join("; ")
}

/// A JSON array of strings.
fn enum_values(values: &[&str]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| Value::String((*value).to_owned()))
            .collect(),
    )
}

/// `` `a`, `b` or `c` ``, for an error message or a description.
fn quoted_list(values: &[&str]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| format!("`{value}`")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

/// Run one tool against a dataset that was verified in this same request.
///
/// `role` comes from the bearer token, resolved server-side. Nothing in
/// `arguments` can change it.
///
/// Every argument name is checked against the tool's own schema *before* any
/// of them is read. Each schema declares `additionalProperties: false`, and
/// a server that read the names it knew and dropped the rest would answer a
/// different assertion from the one the client sent — a `verify_claim` call
/// carrying `"negated": true` would come back SUPPORTED for the claim it did
/// not make. A name this tool did not declare is a JSON-RPC error, so it
/// carries no verdict and cannot be mistaken for one.
pub fn call(
    tool: DeclaredTool,
    arguments: Option<&JsonObject>,
    role: &Role,
    dataset: &VerifiedDataset,
) -> Result<CallToolResult, ErrorData> {
    if let Some(arguments) = arguments {
        reject_undeclared_arguments(tool, arguments)?;
    }
    let result = match tool {
        DeclaredTool::GetMilestones => {
            CallToolResult::structured(answers::get_milestones(dataset, role))
        }
        DeclaredTool::GetCertificates => {
            CallToolResult::structured(answers::get_certificates(dataset, role))
        }
        DeclaredTool::GetPaymentStatus => {
            let unit = required_string(arguments, "unit", tool)?;
            match answers::get_payment_status(dataset, role, unit) {
                Ok(answer) => CallToolResult::structured(answer),
                Err(refused) => CallToolResult::structured_error(refused),
            }
        }
        DeclaredTool::VerifyClaim => {
            let subject_type = SubjectType::parse(one_of(
                arguments,
                "subject_type",
                tool,
                &SubjectType::ALL
                    .iter()
                    .map(|subject_type| subject_type.as_str())
                    .collect::<Vec<&str>>(),
            )?)
            .ok_or_else(|| unreachable_enum(tool, "subject_type"))?;
            let subject_id = required_string(arguments, "subject_id", tool)?;
            let field_name = one_of(arguments, "field", tool, &Field::declared_names())?;
            let asserted_value = required_string(arguments, "asserted_value", tool)?;
            let instalment_id = optional_string(arguments, "instalment_id", tool)?;

            let assertion = Assertion::new(
                subject_type,
                subject_id,
                field_name,
                asserted_value,
                instalment_id,
            );
            CallToolResult::structured(answers::verify_claim(dataset, role, &assertion))
        }
        DeclaredTool::AskProject => {
            let topic = Topic::parse(one_of(
                arguments,
                "topic",
                tool,
                &Topic::ALL
                    .iter()
                    .map(|topic| topic.as_str())
                    .collect::<Vec<&str>>(),
            )?)
            .ok_or_else(|| unreachable_enum(tool, "topic"))?;
            let unit = optional_string(arguments, "unit", tool)?;
            match answers::ask_project(dataset, role, topic, unit) {
                Ok(answer) => CallToolResult::structured(answer),
                Err(refused) => CallToolResult::structured_error(refused),
            }
        }
        DeclaredTool::WhatsChanged => {
            let delivered = match optional_string(arguments, "since_cursor", tool)? {
                Some(cursor) => cursor::decode(cursor).map_err(|error| {
                    ErrorData::invalid_params(
                        format!("`{}` cannot read `since_cursor`: {error}", tool.name()),
                        None,
                    )
                })?,
                None => BTreeSet::new(),
            };
            CallToolResult::structured(answers::whats_changed(dataset, role, &delivered))
        }
    };
    Ok(result)
}

/// Refuse any argument name this tool does not declare.
///
/// The names come from the tool's own schema, so this cannot drift from what
/// the client was told. The first undeclared name is quoted back, clipped on
/// a character boundary: a caller controls it, so it is bounded before it
/// reaches a message, and the count says how many more there were.
fn reject_undeclared_arguments(
    tool: DeclaredTool,
    arguments: &JsonObject,
) -> Result<(), ErrorData> {
    let undeclared: Vec<&str> = arguments
        .keys()
        .map(String::as_str)
        .filter(|name| !tool.declares(name))
        .collect();
    let Some(first) = undeclared.first() else {
        return Ok(());
    };
    let declared = tool.declared_arguments();
    let takes = if declared.is_empty() {
        "it takes no arguments at all".to_owned()
    } else {
        format!("it takes {}", quoted_list(&declared))
    };
    Err(ErrorData::invalid_params(
        format!(
            "`{}` does not declare an argument `{}`{}; {takes}, its schema is closed \
             (`additionalProperties: false`), and this server reads no name it did not \
             declare rather than answering a call other than the one you sent",
            tool.name(),
            clipped(first),
            if undeclared.len() > 1 {
                format!(" (and {} more it does not declare)", undeclared.len() - 1)
            } else {
                String::new()
            },
        ),
        None,
    ))
}

/// At most [`MAX_ECHOED_NAME_CHARS`] characters of a name the caller chose.
///
/// Clipped by characters, never by bytes: slicing a byte offset panics on the
/// first non-ASCII argument name, and a panic in a handler takes the server
/// down.
fn clipped(name: &str) -> String {
    let mut clipped: String = name.chars().take(MAX_ECHOED_NAME_CHARS).collect();
    if name.chars().nth(MAX_ECHOED_NAME_CHARS).is_some() {
        clipped.push('…');
    }
    clipped
}

/// How much of a caller's own argument name an error message repeats.
const MAX_ECHOED_NAME_CHARS: usize = 64;

/// The result every tool returns when the record itself cannot be trusted.
///
/// The record carries the milestones, the payments and the escrow balance, so
/// a tool that answered while `PROJECT-RECORD` was unverified would be
/// answering from bytes nobody signed. Every tool refuses together.
///
/// What the refusal *says* is role-filtered like every other answer: the
/// detail of a dataset-level fault names the file that broke it, and a buyer
/// told that `docs/contractor-note-CN-14.md` is unsigned has been told an
/// internal document exists. They get the count and the kind; the detail is
/// logged where the operator who can act on it will see it.
pub fn refusal(tool: DeclaredTool, role: &Role, reason: &DatasetUnavailable) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "status": reason.code(),
        "tool": tool.name(),
        "role": role.to_string(),
        "message": format!(
            "This server will not answer `{}`: {}. Every tool refuses together while the \
             signed project record does not verify, because every fact this server states comes \
             from it.",
            tool.name(),
            reason.message_for(role),
        ),
    }))
}

/// Read a required string argument, trimmed, or say exactly what was expected.
///
/// Surrounding whitespace is removed here and nowhere else: it is the one step
/// applied before any value is read, so a stray space is never the difference
/// between two values.
fn required_string<'a>(
    arguments: Option<&'a JsonObject>,
    name: &str,
    tool: DeclaredTool,
) -> Result<&'a str, ErrorData> {
    match arguments.and_then(|arguments| arguments.get(name)) {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.trim()),
        Some(Value::String(_)) | None => Err(ErrorData::invalid_params(
            format!(
                "`{}` needs a non-empty string argument `{name}`",
                tool.name()
            ),
            None,
        )),
        Some(other) => Err(mistyped(tool, name, other)),
    }
}

/// Read an optional string argument, trimmed.
///
/// Absent and `null` both mean "not given". An empty string does not: a caller
/// who sent the argument meant to say something, and an empty value says
/// nothing, so it is a mistake worth naming rather than silently ignoring.
fn optional_string<'a>(
    arguments: Option<&'a JsonObject>,
    name: &str,
    tool: DeclaredTool,
) -> Result<Option<&'a str>, ErrorData> {
    match arguments.and_then(|arguments| arguments.get(name)) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim())),
        Some(Value::String(_)) => Err(ErrorData::invalid_params(
            format!(
                "`{}` takes `{name}` as a non-empty string, or not at all",
                tool.name()
            ),
            None,
        )),
        Some(other) => Err(mistyped(tool, name, other)),
    }
}

/// Read a required argument whose value must be one of a closed set.
///
/// The returned string is the declared one, not the caller's copy of it, so
/// everything downstream works with `&'static str` from this server's own
/// vocabulary.
fn one_of(
    arguments: Option<&JsonObject>,
    name: &str,
    tool: DeclaredTool,
    permitted: &[&'static str],
) -> Result<&'static str, ErrorData> {
    let sent = required_string(arguments, name, tool)?;
    permitted
        .iter()
        .copied()
        .find(|value| *value == sent)
        .ok_or_else(|| {
            ErrorData::invalid_params(
                format!(
                    "`{}` takes `{name}` as one of {}; it is a closed set and nothing outside \
                     it is interpreted",
                    tool.name(),
                    quoted_list(permitted),
                ),
                None,
            )
        })
}

/// A value that was one of the declared words a moment ago and is not now.
///
/// [`one_of`] returns only strings from the same list these enums are built
/// from, so this cannot happen; it is an error rather than a panic because a
/// handler that panics takes the server down, and this one is reachable only
/// if the two lists ever stop agreeing.
fn unreachable_enum(tool: DeclaredTool, name: &str) -> ErrorData {
    ErrorData::internal_error(
        format!(
            "`{}` declared a value for `{name}` that it cannot itself read; this build's tool \
             schema and its argument reader disagree",
            tool.name(),
        ),
        None,
    )
}

/// An argument of the wrong JSON type, named along with what arrived.
fn mistyped(tool: DeclaredTool, name: &str, value: &Value) -> ErrorData {
    ErrorData::invalid_params(
        format!(
            "`{}` needs `{name}` to be a string, but it was {}",
            tool.name(),
            json_type_of(value),
        ),
        None,
    )
}

/// The JSON type name of a value, for an error message that names what arrived.
fn json_type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::DocumentStatus;

    fn arguments(pairs: &[(&str, Value)]) -> JsonObject {
        let mut object = JsonObject::new();
        for (name, value) in pairs {
            object.insert((*name).to_owned(), value.clone());
        }
        object
    }

    fn schema_of(name: &str) -> &'static Arc<JsonObject> {
        DeclaredTool::from_name(name)
            .unwrap_or_else(|| panic!("`{name}` is not declared"))
            .schema()
    }

    fn strings(value: Option<&Value>) -> Vec<String> {
        value
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn the_six_tool_names_are_declared_exactly_once_each() {
        let declared: Vec<String> = declared_tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();

        let mut sorted = declared.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            declared.len(),
            "a tool name is declared more than once: {declared:?}",
        );

        assert_eq!(
            declared,
            [
                "get_milestones",
                "get_certificates",
                "get_payment_status",
                "verify_claim",
                "ask_project",
                "whats_changed",
            ],
        );
    }

    #[test]
    fn every_declaration_carries_a_description_and_a_closed_object_schema() {
        for tool in declared_tools() {
            let description = tool
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("`{}` has no description", tool.name));
            assert!(
                description.len() > 60,
                "`{}` has a stub description: {description}",
                tool.name,
            );
            assert_eq!(
                tool.input_schema.get("type"),
                Some(&Value::String("object".to_owned())),
                "`{}` does not declare an object schema",
                tool.name,
            );
            assert_eq!(
                tool.input_schema.get("additionalProperties"),
                Some(&Value::Bool(false)),
                "`{}` accepts arguments it does not declare",
                tool.name,
            );
        }
    }

    /// The whole point of the rework: no tool asks for prose.
    #[test]
    fn no_tool_takes_a_free_text_claim_or_question() {
        for tool in declared_tools() {
            let properties = tool
                .input_schema
                .get("properties")
                .and_then(Value::as_object);
            let declared: Vec<&String> = properties
                .map(|map| map.keys().collect())
                .unwrap_or_default();
            for prose in ["claim", "question", "text", "sentence", "query"] {
                assert!(
                    !declared.iter().any(|name| name.as_str() == prose),
                    "`{}` still takes a free-text `{prose}`: {declared:?}",
                    tool.name,
                );
            }
        }
    }

    #[test]
    fn the_tools_that_take_an_argument_require_it_by_name() {
        let required = |name: &str| -> Vec<String> { strings(schema_of(name).get("required")) };

        assert_eq!(required("get_payment_status"), ["unit"]);
        assert_eq!(
            required("verify_claim"),
            ["subject_type", "subject_id", "field", "asserted_value"],
        );
        assert_eq!(required("ask_project"), ["topic"]);
        assert!(required("whats_changed").is_empty());
        assert!(required("get_milestones").is_empty());
        assert!(required("get_certificates").is_empty());
    }

    #[test]
    fn the_closed_enums_are_declared_in_the_schema() {
        let schema = schema_of("verify_claim");
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("verify_claim declares properties");

        assert_eq!(
            strings(properties["subject_type"].get("enum")),
            ["unit", "certificate", "milestone"],
        );
        let fields = strings(properties["field"].get("enum"));
        assert_eq!(fields.len(), Field::declared_names().len());
        assert!(
            fields.contains(&"paid_to_date_minor".to_owned()),
            "{fields:?}"
        );
        assert!(properties["asserted_value"].get("enum").is_none());

        assert_eq!(
            strings(
                schema_of("ask_project")
                    .get("properties")
                    .and_then(Value::as_object)
                    .expect("ask_project declares properties")["topic"]
                    .get("enum")
            ),
            [
                "overview",
                "milestones",
                "certificates",
                "payments",
                "escrow"
            ],
        );
    }

    /// The schema says which field names go with which subject type, and
    /// which words each status field takes, so a client that validates
    /// cannot send a mismatched pair or a status in a spelling this server
    /// does not read.
    ///
    /// The `asserted_value` clauses are new, and they are the schema half of
    /// the r3 review's R1: the vocabularies were nowhere in the schema, so a
    /// caller could learn that a milestone's status is spelled `in_progress`
    /// only from a refusal — and the ones who guessed `In_progress` were
    /// told their true claim was contradicted. Each clause is checked
    /// against `FieldKind::vocabulary`, the list the server itself reads
    /// values with, so the schema cannot advertise words it will not accept.
    #[test]
    fn the_schema_narrows_field_by_subject_type_and_a_status_by_its_vocabulary() {
        let schema = schema_of("verify_claim");
        let narrowed = schema
            .get("allOf")
            .and_then(Value::as_array)
            .expect("verify_claim narrows `field` per subject type");

        // A clause that pins `field` to one value narrows `asserted_value`;
        // the rest narrow `field` itself, one per subject type.
        let by_subject_type: Vec<&Value> = narrowed
            .iter()
            .filter(|clause| clause["if"]["properties"]["field"]["const"].is_null())
            .collect();
        assert_eq!(by_subject_type.len(), SubjectType::ALL.len());

        let certificate = by_subject_type
            .iter()
            .find(|clause| clause["if"]["properties"]["subject_type"]["const"] == "certificate")
            .expect("the certificate clause is declared");
        let permitted = strings(certificate["then"]["properties"]["field"].get("enum"));
        assert!(
            permitted.contains(&"issued_date".to_owned()),
            "{permitted:?}"
        );
        assert!(
            !permitted.contains(&"paid_to_date_minor".to_owned()),
            "a certificate has no payment field: {permitted:?}",
        );

        let mut vocabularies = 0usize;
        for subject_type in SubjectType::ALL {
            for field in subject_type.fields() {
                let Some(words) = field.kind().vocabulary() else {
                    continue;
                };
                vocabularies += 1;
                let clause = narrowed
                    .iter()
                    .find(|clause| {
                        clause["if"]["properties"]["subject_type"]["const"] == subject_type.as_str()
                            && clause["if"]["properties"]["field"]["const"] == field.as_str()
                    })
                    .unwrap_or_else(|| {
                        panic!(
                            "the schema never declares the words `{}` `{}` is read with, so a \
                             caller can only learn them from a refusal: {narrowed:?}",
                            subject_type.as_str(),
                            field.as_str(),
                        )
                    });
                assert_eq!(
                    strings(clause["then"]["properties"]["asserted_value"].get("enum")),
                    words
                        .iter()
                        .map(|word| (*word).to_owned())
                        .collect::<Vec<String>>(),
                    "`{}` `{}` is advertised with words it is not read with",
                    subject_type.as_str(),
                    field.as_str(),
                );
            }
        }
        assert_eq!(
            vocabularies, 3,
            "a milestone's status, a certificate's and an instalment's",
        );
        assert_eq!(narrowed.len(), SubjectType::ALL.len() + vocabularies);
    }

    #[test]
    fn a_value_outside_a_closed_enum_is_a_schema_error_naming_the_set() {
        let sent = arguments(&[("subject_type", Value::String("escrow".to_owned()))]);
        let error = one_of(
            Some(&sent),
            "subject_type",
            DeclaredTool::VerifyClaim,
            &["unit", "certificate", "milestone"],
        )
        .expect_err("`escrow` is not a subject type");
        assert_eq!(error.code.0, -32602, "{error:?}");
        assert!(
            error
                .message
                .contains("`unit`, `certificate` or `milestone`")
                && error.message.contains("closed set"),
            "{}",
            error.message,
        );
        assert!(
            !error.message.contains("verdict") && !error.message.contains("UNVERIFIED"),
            "a schema error must not read like a verdict: {}",
            error.message,
        );
    }

    #[test]
    fn a_missing_or_mistyped_argument_names_what_was_expected() {
        let error = required_string(None, "unit", DeclaredTool::GetPaymentStatus)
            .expect_err("an absent argument is an error");
        assert!(
            error.message.contains("get_payment_status")
                && error.message.contains("non-empty string argument `unit`"),
            "{}",
            error.message,
        );

        let numeric = arguments(&[("unit", Value::Number(1204.into()))]);
        let error = required_string(Some(&numeric), "unit", DeclaredTool::GetPaymentStatus)
            .expect_err("a number is not a unit");
        assert!(error.message.contains("a number"), "{}", error.message);

        let blank = arguments(&[("unit", Value::String("   ".to_owned()))]);
        assert!(
            required_string(Some(&blank), "unit", DeclaredTool::GetPaymentStatus).is_err(),
            "whitespace is not a unit number",
        );

        let padded = arguments(&[("unit", Value::String("  1204 ".to_owned()))]);
        assert_eq!(
            required_string(Some(&padded), "unit", DeclaredTool::GetPaymentStatus).ok(),
            Some("1204"),
            "surrounding whitespace is removed once, here",
        );
    }

    #[test]
    fn an_optional_argument_is_absent_null_or_a_real_value() {
        let tool = DeclaredTool::WhatsChanged;
        assert_eq!(optional_string(None, "since_cursor", tool), Ok(None));

        let null = arguments(&[("since_cursor", Value::Null)]);
        assert_eq!(optional_string(Some(&null), "since_cursor", tool), Ok(None));

        let given = arguments(&[("since_cursor", Value::String(" v1.61 ".to_owned()))]);
        assert_eq!(
            optional_string(Some(&given), "since_cursor", tool),
            Ok(Some("v1.61")),
        );

        let empty = arguments(&[("since_cursor", Value::String("".to_owned()))]);
        let error = optional_string(Some(&empty), "since_cursor", tool)
            .expect_err("an empty optional argument is a mistake worth naming");
        assert!(error.message.contains("or not at all"), "{}", error.message);
    }

    #[test]
    fn from_name_round_trips_and_rejects_anything_else() {
        for tool in DeclaredTool::ALL {
            assert_eq!(DeclaredTool::from_name(tool.name()), Some(tool));
        }
        assert_eq!(DeclaredTool::from_name("get_milestone"), None);
        assert_eq!(DeclaredTool::from_name(""), None);
    }

    #[test]
    fn the_refusal_names_the_record_and_carries_no_project_data() {
        let result = refusal(
            DeclaredTool::GetMilestones,
            &Role::Buyer {
                unit: "1204".to_owned(),
            },
            &DatasetUnavailable::ProjectRecordUnverified {
                status: DocumentStatus::SignatureInvalid,
                detail: "hash: PROJECT-RECORD (project.json) — the manifest records sha256 aa, \
                         but the file on disk hashes to bb"
                    .to_owned(),
            },
        );

        assert_eq!(result.is_error, Some(true));
        let structured = result
            .structured_content
            .as_ref()
            .expect("the refusal carries structured content");
        assert_eq!(structured["status"], "project_record_unverified");
        assert_eq!(structured["tool"], "get_milestones");
        assert_eq!(structured["role"], "buyer:1204");

        let rendered = structured.to_string();
        assert!(rendered.contains("signature_invalid"), "{rendered}");
        for forbidden in ["escrow", "instalment", "AED", "percent_complete"] {
            assert!(
                !rendered.contains(forbidden),
                "a refusal must not leak project data: {rendered}",
            );
        }
    }
}
