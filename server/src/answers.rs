//! What each tool answers, composed from a verification run that just happened.
//!
//! Three rules hold everywhere in this module.
//!
//! 1. **Every fact carries the `doc_id`s that evidence it.** A statement whose
//!    supporting documents are all unreadable, unverified or outside the
//!    caller's role is not returned at all; it is listed as withheld, with the
//!    reason.
//! 2. **Role is read from [`Role`], never from an argument.** A document the
//!    caller may not see is left out silently — naming it, even to say it was
//!    withheld, would tell a buyer that an internal document exists. The same
//!    rule governs the counts in every answer: a buyer is told how many
//!    documents *a buyer* has, so a developer-only document failing cannot be
//!    read off a buyer's totals.
//! 3. **Nothing here re-checks a signature.** The statuses come from the
//!    [`VerifiedDataset`] that this same request produced in [`crate::verify`]
//!    a few microseconds earlier.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    assertion::{self, Assertion, Reading, SubjectType},
    auth::Role,
    card, cursor,
    record::{
        Certificate, Milestone, PROJECT_RECORD_DOC_ID, PaymentSchedule, ProjectRecord, Visibility,
    },
    verify::{DocumentEvidence, VerifiedDataset},
};

/// The date `whats_changed` measures a caller without a cursor from.
///
/// A caller who sends no cursor is told about everything the record dates
/// after this day. It is a fixed seed, so the first answer is the same for
/// every caller and every run rather than depending on a clock.
pub const CHANGE_BASELINE_DATE: &str = "2026-09-01";

/// Collects the documents behind the facts in one answer.
///
/// It keeps two lists: documents that may be cited to this caller (visible to
/// them *and* verified in this run) and documents the caller may see but which
/// did not verify. Documents outside the caller's role never enter either list.
struct DocumentIndex<'a> {
    dataset: &'a VerifiedDataset,
    role: &'a Role,
    cited: BTreeMap<String, Value>,
    unverified: BTreeMap<String, Value>,
}

impl<'a> DocumentIndex<'a> {
    fn new(dataset: &'a VerifiedDataset, role: &'a Role) -> Self {
        Self {
            dataset,
            role,
            cited: BTreeMap::new(),
            unverified: BTreeMap::new(),
        }
    }

    /// Record the evidence behind one fact and return the ids that may be
    /// cited for it. An empty return means the fact must not be stated.
    fn cite<I, S>(&mut self, doc_ids: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let dataset = self.dataset;
        let role = self.role;
        let mut citable = Vec::new();
        for doc_id in doc_ids {
            let doc_id = doc_id.as_ref();
            let Some(evidence) = dataset.document(doc_id) else {
                // The record points at a document it does not declare. Nothing
                // was signed for it, so it evidences nothing, and `citable`
                // never gains the id — the fact resting on it is withheld from
                // everyone, whatever their role.
                //
                // Who may be *told* that is a separate question, and it cannot
                // be answered the way the branch below answers it: there is no
                // declaration, so there is no `visibility` to check. An
                // undeclared id is therefore read as developer-only, which is
                // what `visible_to` answers for an internal document and the
                // only answer that fails closed. A signed record citing
                // `CN-14` from a buyer-visible milestone would otherwise put
                // that name in a buyer's reply and tell them an internal
                // document exists — rule 2 of this module, which a refusal and
                // an `unverified` note are no more exempt from than an answer
                // is. The developer is the operator who can fix the record and
                // is told exactly which id dangles.
                if matches!(role, Role::Developer) {
                    self.unverified.insert(
                        doc_id.to_owned(),
                        json!({
                            "doc_id": doc_id,
                            "status": "missing",
                            "detail": format!(
                                "the project record cites {doc_id} but does not declare it, so \
                                 no signature was checked for it"
                            ),
                        }),
                    );
                }
                continue;
            };
            if !evidence.visibility.visible_to(role) {
                continue;
            }
            if evidence.citable_to(role) {
                self.cited
                    .insert(doc_id.to_owned(), citation(evidence, dataset));
                citable.push(doc_id.to_owned());
            } else {
                self.unverified
                    .insert(doc_id.to_owned(), unverified_note(evidence));
            }
        }
        citable
    }

    /// Whether this caller may see a document at all.
    fn visible(&self, doc_id: &str) -> bool {
        self.dataset
            .document(doc_id)
            .is_some_and(|evidence| evidence.visibility.visible_to(self.role))
    }

    fn documents(&self) -> Vec<Value> {
        self.cited.values().cloned().collect()
    }

    fn unverified(&self) -> Vec<Value> {
        self.unverified.values().cloned().collect()
    }
}

/// One cited document, as a caller sees it.
fn citation(evidence: &DocumentEvidence, dataset: &VerifiedDataset) -> Value {
    json!({
        "doc_id": evidence.doc_id,
        "title": evidence.title,
        "path": evidence.path,
        "issuer": evidence.issuer_name,
        "issuer_id": evidence.issuer_id,
        "sha256": evidence.sha256,
        "date": dataset.record.document_date(&evidence.doc_id),
        "status": evidence.status.as_str(),
        "evidence_card": card::card_uri(&evidence.doc_id),
    })
}

/// One document the caller may see but which this run could not stand behind.
fn unverified_note(evidence: &DocumentEvidence) -> Value {
    json!({
        "doc_id": evidence.doc_id,
        "title": evidence.title,
        "status": evidence.status.as_str(),
        "detail": evidence.detail,
    })
}

/// The verification run itself, reported with every answer.
///
/// **What this does and does not promise.** A fault belonging to the dataset
/// as a whole — a repeated `doc_id`, a bad manifest header, a file signed but
/// not declared — refuses every call in [`crate::verify`], so no answer is
/// ever composed from one. A fault belonging to a *single document* does not:
/// that document is reported as `signature_invalid` or `missing`, nothing is
/// stated on its evidence, and every other document still answers. So an
/// answer can exist while `documents_verified` is less than
/// `documents_declared`, and when it does, the failing document is named in
/// the `unverified` list beside it. `verify-dataset` exits non-zero in both
/// cases, because its job is "is this dataset entirely sound"; this server
/// degrades per document on purpose, because a buyer whose own schedule
/// verified should still hear about it.
///
/// The counts are what *this caller* may see. A buyer is not told that 14
/// documents exist when 10 of them are theirs to read, and a developer-only
/// document failing does not show up in a buyer's totals as a missing one.
fn verification(dataset: &VerifiedDataset, role: &Role) -> Value {
    let (declared, verified) = dataset.counts_for(role);
    json!({
        "dataset": dataset.dataset,
        "documents_declared": declared,
        "documents_verified": verified,
        "scope": "these counts are the documents your role may read, not the whole dataset",
        "checked": "every status in this result was produced by recomputing each document's \
                    hash and re-checking its Ed25519 signature during this call",
        "degrades": "a document that fails is reported as such and nothing is stated on its \
                     evidence; a fault belonging to the dataset itself refuses every call \
                     instead",
    })
}

/// The documents that may evidence one milestone for this caller, and which of
/// them is the internal contractor note.
///
/// The note joins the evidence only for a role that may read it, which is the
/// single place that rule lives: `get_milestones`, `ask_project`,
/// `verify_claim` and `whats_changed` all come through here.
fn milestone_evidence(
    milestone: &Milestone,
    index: &DocumentIndex<'_>,
) -> (Vec<String>, Option<String>) {
    let note = milestone
        .contractor_note
        .as_ref()
        .filter(|note| index.visible(note))
        .cloned();
    let mut evidence_ids = milestone.evidence.clone();
    if let Some(note) = &note {
        evidence_ids.push(note.clone());
    }
    (evidence_ids, note)
}

/// Milestones, with the documents that evidence each one.
pub fn get_milestones(dataset: &VerifiedDataset, role: &Role) -> Value {
    let record = &dataset.record;
    let mut index = DocumentIndex::new(dataset, role);
    let mut milestones = Vec::new();
    let mut withheld = Vec::new();

    for milestone in &record.milestones {
        if !Visibility::parse(&milestone.visibility).visible_to(role) {
            continue;
        }
        let (evidence_ids, contractor_note) = milestone_evidence(milestone, &index);
        let any_visible = evidence_ids.iter().any(|id| index.visible(id));
        let citations = index.cite(evidence_ids);

        if citations.is_empty() {
            withheld.push(json!({
                "id": milestone.id,
                "title": milestone.title,
                "reason": if any_visible {
                    "every source document that evidences this milestone failed verification in \
                     this call"
                } else {
                    "no source document in this view of the record evidences this milestone"
                },
            }));
            continue;
        }

        milestones.push(json!({
            "id": milestone.id,
            "title": milestone.title,
            "status": milestone.status,
            "planned_date": milestone.planned_date,
            "actual_date": milestone.actual_date,
            "percent_complete": milestone.percent_complete,
            "percent_of_works": milestone.percent_of_works,
            "contractor_note": contractor_note,
            "statement": milestone_statement(milestone, &record.project.as_of),
            "citations": citations,
        }));
    }

    json!({
        "project": record.project.name,
        "as_of": record.project.as_of,
        "role": role.to_string(),
        "milestones": milestones,
        "withheld": withheld,
        "documents": index.documents(),
        "unverified": index.unverified(),
        "verification": verification(dataset, role),
    })
}

/// Certificates, each with the result of checking its document in this call.
pub fn get_certificates(dataset: &VerifiedDataset, role: &Role) -> Value {
    let record = &dataset.record;
    let mut index = DocumentIndex::new(dataset, role);
    let mut certificates = Vec::new();

    for certificate in visible_certificates(record, role) {
        if !index.visible(&certificate.document) {
            continue;
        }

        let (status, detail) = match dataset.document(&certificate.document) {
            Some(evidence) => (evidence.status.as_str(), evidence.detail.clone()),
            None => (
                "missing",
                format!(
                    "the project record cites {} but does not declare it",
                    certificate.document
                ),
            ),
        };
        let citations = index.cite([PROJECT_RECORD_DOC_ID, certificate.document.as_str()]);

        certificates.push(json!({
            "id": certificate.id,
            "title": certificate.title,
            "issuer": record.issuer_name(&certificate.issuer),
            "issuer_id": certificate.issuer,
            "record_status": certificate.status,
            "issued_date": certificate.issued_date,
            "expires_date": certificate.expires_date,
            "scope": certificate.scope,
            "document": certificate.document,
            "signature": {
                "status": status,
                "detail": if detail.is_empty() { Value::Null } else { Value::String(detail) },
            },
            "may_be_relied_on": status == "verified",
            "statement": certificate_statement(certificate, record, status),
            "citations": citations,
            "evidence_card": card::card_uri(&certificate.document),
        }));
    }

    json!({
        "project": record.project.name,
        "as_of": record.project.as_of,
        "role": role.to_string(),
        "certificates": certificates,
        "documents": index.documents(),
        "unverified": index.unverified(),
        "verification": verification(dataset, role),
    })
}

/// One sentence about a certificate that stays true when its bytes have moved.
fn certificate_statement(
    certificate: &Certificate,
    record: &ProjectRecord,
    signature_status: &str,
) -> String {
    let issuer = record.issuer_name(&certificate.issuer);
    let dated = match &certificate.issued_date {
        Some(date) => format!(" on {date}"),
        None => String::new(),
    };
    if signature_status == "verified" {
        format!(
            "The project record records certificate {} as {} by {issuer}{dated}, and its stored \
             document verified in this call.",
            certificate.id, certificate.status,
        )
    } else {
        format!(
            "The project record records certificate {} as {} by {issuer}{dated}, but its stored \
             document did not verify in this call ({signature_status}), so this server does not \
             state it as held evidence.",
            certificate.id, certificate.status,
        )
    }
}

/// Instalments and escrow for one unit.
///
/// `Err` is the refusal a buyer gets for any unit but their own.
pub fn get_payment_status(
    dataset: &VerifiedDataset,
    role: &Role,
    unit: &str,
) -> Result<Value, Value> {
    if let Role::Buyer { unit: own } = role
        && own.as_str() != unit
    {
        return Err(role_scope_refusal(own));
    }

    let record = &dataset.record;
    let Some(schedule) = record.schedule_for(unit) else {
        return Err(json!({
            "status": "not_found",
            "message": "The project record holds no payment schedule for the unit this call \
                        named.",
        }));
    };

    let mut index = DocumentIndex::new(dataset, role);
    let citations = index.cite([PROJECT_RECORD_DOC_ID, schedule.document.as_str()]);
    if citations.is_empty() {
        return Err(json!({
            "status": "unverifiable",
            "message": format!(
                "The payment schedule for unit {} cannot be stated: no document that \
                 evidences it verified in this call.",
                schedule.unit,
            ),
            "unverified": index.unverified(),
            "verification": verification(dataset, role),
        }));
    }

    let currency = &record.currency;
    let instalments: Vec<Value> = schedule
        .instalments
        .iter()
        .map(|instalment| {
            json!({
                "id": instalment.id,
                "label": instalment.label,
                "trigger": instalment.trigger,
                "percent": instalment.percent,
                "amount_minor": instalment.amount_minor,
                "amount": currency.format_minor(instalment.amount_minor),
                "due_date": instalment.due_date,
                "status": instalment.status,
                "paid_date": instalment.paid_date,
            })
        })
        .collect();
    let escrow = escrow_block(dataset, &mut index);
    let outstanding = assertion::outstanding_minor(schedule);

    Ok(json!({
        "project": record.project.name,
        "as_of": record.project.as_of,
        "role": role.to_string(),
        "unit": schedule.unit,
        "unit_type": schedule.unit_type,
        "currency": currency.code,
        "currency_minor_units_per_major": currency.minor_units_per_major,
        "total_price_minor": schedule.total_price_minor,
        "total_price": currency.format_minor(schedule.total_price_minor),
        "paid_to_date_minor": schedule.paid_to_date_minor,
        "paid_to_date": currency.format_minor(schedule.paid_to_date_minor),
        // The contract price less what has been paid, by the same integer
        // arithmetic as everything else here: the question a buyer actually
        // asks, answered without anyone subtracting two decimal strings.
        "outstanding_minor": outstanding,
        "outstanding": outstanding.map(|amount| currency.format_minor(amount)),
        "has_overdue_instalment": assertion::has_overdue(schedule),
        "escrow_account_ref": schedule.escrow_account_ref,
        "instalments": instalments,
        "citations": citations,
        "escrow": escrow,
        "documents": index.documents(),
        "unverified": index.unverified(),
        "verification": verification(dataset, role),
        "note": "Amounts are integers in minor units; the formatted strings are produced from \
                 those integers by integer arithmetic and are never parsed as decimals.",
    }))
}

/// The refusal a buyer gets for any unit but their own.
///
/// It names the unit the *token* is bound to and nothing the caller sent. A
/// refusal that repeated the argument back would be a refusal that states
/// whatever the caller put in it, which is not this server's to state.
fn role_scope_refusal(own: &str) -> Value {
    json!({
        "status": "refused",
        "reason": "role_scope",
        "message": format!(
            "This bearer token is bound to unit {own}. The server resolves that from the token, \
             not from the request, and will not read any other unit's payment schedule with it."
        ),
    })
}

/// The escrow block, which states a balance only when its attestation verified.
fn escrow_block(dataset: &VerifiedDataset, index: &mut DocumentIndex<'_>) -> Value {
    let record = &dataset.record;
    let escrow = &record.escrow;
    let citations = index.cite([PROJECT_RECORD_DOC_ID, escrow.latest_attestation.as_str()]);
    if citations.iter().any(|id| id == &escrow.latest_attestation) {
        json!({
            "status": "attested",
            "account_ref": escrow.account_ref,
            "trustee": record.issuer_name(&escrow.trustee),
            "attested_balance_minor": escrow.attested_balance_minor,
            "attested_balance": record.currency.format_minor(escrow.attested_balance_minor),
            "attested_as_of": escrow.attested_as_of,
            "citations": citations,
        })
    } else {
        json!({
            "status": "unverified",
            "account_ref": escrow.account_ref,
            "trustee": record.issuer_name(&escrow.trustee),
            "attestation_document": escrow.latest_attestation,
            "message": "The balance is not stated: the attestation document behind it did not \
                        verify in this call.",
            "citations": citations,
        })
    }
}

/// What the record says about one subject, with the evidence behind it.
struct Fact {
    statement: String,
    citations: Vec<String>,
}

impl Fact {
    fn render(&self) -> Value {
        json!({"statement": self.statement, "citations": self.citations})
    }
}

/// The subjects `ask_project` can be asked for.
///
/// A closed set, chosen by the caller. Nothing here is inferred from words:
/// the previous build matched keywords against a question, which meant a
/// question it did not recognise was answered about whatever it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    /// The project's own status and handover date.
    Overview,
    /// Construction milestones.
    Milestones,
    /// Certificates and what they attest.
    Certificates,
    /// One or more units' instalments.
    Payments,
    /// The escrow account and its latest attestation.
    Escrow,
}

impl Topic {
    /// Every topic, in the order the tool schema lists them.
    pub const ALL: [Topic; 5] = [
        Topic::Overview,
        Topic::Milestones,
        Topic::Certificates,
        Topic::Payments,
        Topic::Escrow,
    ];

    /// The word a caller sends.
    pub const fn as_str(self) -> &'static str {
        match self {
            Topic::Overview => "overview",
            Topic::Milestones => "milestones",
            Topic::Certificates => "certificates",
            Topic::Payments => "payments",
            Topic::Escrow => "escrow",
        }
    }

    /// Read a caller's `topic`. `None` is a value outside the enum.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|topic| topic.as_str() == value)
    }
}

/// Compose what the record says on one topic, citing every statement.
///
/// `schedules` is the set of units the answer may speak about, resolved from
/// the caller's token and the unit they named. Nothing here chooses a unit on
/// its own, which is what stops a question about one unit being answered about
/// another.
fn facts_for(
    dataset: &VerifiedDataset,
    role: &Role,
    topic: Topic,
    schedules: &[&PaymentSchedule],
    index: &mut DocumentIndex<'_>,
) -> Vec<Fact> {
    let record = &dataset.record;
    let mut facts = Vec::new();

    match topic {
        Topic::Overview => {
            let citations = index.cite([PROJECT_RECORD_DOC_ID]);
            if !citations.is_empty() {
                facts.push(Fact {
                    statement: format!(
                        "The project record gives an expected handover of {} and records the \
                         project as {} as at {}.",
                        record.project.expected_handover,
                        record.project.status,
                        record.project.as_of,
                    ),
                    citations,
                });
            }
        }
        Topic::Milestones => {
            for milestone in visible_milestones(record, role) {
                if milestone.status == "not_started" {
                    continue;
                }
                let (evidence_ids, _note) = milestone_evidence(milestone, index);
                let citations = index.cite(evidence_ids);
                if citations.is_empty() {
                    continue;
                }
                facts.push(Fact {
                    statement: milestone_statement(milestone, &record.project.as_of),
                    citations,
                });
            }
        }
        Topic::Certificates => {
            for certificate in visible_certificates(record, role) {
                let status = dataset
                    .document(&certificate.document)
                    .map_or("missing", |evidence| evidence.status.as_str());
                let citations = index.cite([PROJECT_RECORD_DOC_ID, certificate.document.as_str()]);
                if !citations.iter().any(|id| id == &certificate.document) {
                    continue;
                }
                facts.push(Fact {
                    statement: certificate_statement(certificate, record, status),
                    citations,
                });
            }
        }
        Topic::Payments => {
            for schedule in schedules {
                let citations = index.cite([PROJECT_RECORD_DOC_ID, schedule.document.as_str()]);
                if !citations.iter().any(|id| id == &schedule.document) {
                    continue;
                }
                facts.push(Fact {
                    statement: format!(
                        "Unit {} has paid {} of {} under its schedule.",
                        schedule.unit,
                        record.currency.format_minor(schedule.paid_to_date_minor),
                        record.currency.format_minor(schedule.total_price_minor),
                    ),
                    citations: citations.clone(),
                });
                if let Some(next) = next_unpaid(schedule) {
                    facts.push(Fact {
                        statement: format!(
                            "The next instalment for unit {} is {} ({}), {}, due {}, recorded \
                             as {}.",
                            schedule.unit,
                            next.id,
                            next.label,
                            record.currency.format_minor(next.amount_minor),
                            next.due_date,
                            next.status,
                        ),
                        citations,
                    });
                }
            }
        }
        Topic::Escrow => {
            let escrow = &record.escrow;
            let citations = index.cite([PROJECT_RECORD_DOC_ID, escrow.latest_attestation.as_str()]);
            if citations.iter().any(|id| id == &escrow.latest_attestation) {
                facts.push(Fact {
                    statement: format!(
                        "Escrow account {} was attested at {} as at {} by {}.",
                        escrow.account_ref,
                        record.currency.format_minor(escrow.attested_balance_minor),
                        escrow.attested_as_of,
                        record.issuer_name(&escrow.trustee),
                    ),
                    citations,
                });
            }
        }
    }

    facts
}

/// The first instalment of a schedule that has no payment date.
fn next_unpaid(schedule: &PaymentSchedule) -> Option<&crate::record::Instalment> {
    schedule
        .instalments
        .iter()
        .find(|instalment| instalment.paid_date.is_none())
}

fn visible_milestones<'a>(record: &'a ProjectRecord, role: &Role) -> Vec<&'a Milestone> {
    record
        .milestones
        .iter()
        .filter(|milestone| Visibility::parse(&milestone.visibility).visible_to(role))
        .collect()
}

/// The certificates a caller may hear about at all.
///
/// A buyer hears about issued certificates only; the developer also hears
/// about the ones that are pending or no longer issued.
fn visible_certificates<'a>(record: &'a ProjectRecord, role: &Role) -> Vec<&'a Certificate> {
    record
        .certificates
        .iter()
        .filter(|certificate| {
            Visibility::parse(&certificate.visibility).visible_to(role)
                && (matches!(role, Role::Developer) || certificate.status == "issued")
        })
        .collect()
}

/// The units a caller may read at all.
fn schedules_in_scope<'a>(record: &'a ProjectRecord, role: &Role) -> Vec<&'a PaymentSchedule> {
    record
        .payment_schedules
        .iter()
        .filter(|schedule| match role {
            Role::Developer => true,
            Role::Buyer { unit } => &schedule.unit == unit,
        })
        .collect()
}

/// The schedules an answer may speak about, or the refusal it earns.
///
/// A unit the caller named is checked against the token first: naming a unit
/// outside it is refused outright, never substituted. Naming none is not a
/// substitution — it is the caller saying "mine", and for a buyer that is one
/// schedule, for the developer every one.
fn schedules_asked_about<'a>(
    record: &'a ProjectRecord,
    role: &Role,
    unit: Option<&str>,
) -> Result<Vec<&'a PaymentSchedule>, Value> {
    let Some(unit) = unit else {
        return Ok(schedules_in_scope(record, role));
    };
    if let Role::Buyer { unit: own } = role
        && own.as_str() != unit
    {
        return Err(role_scope_refusal(own));
    }
    match record.schedule_for(unit) {
        Some(schedule) => Ok(vec![schedule]),
        None => Err(json!({
            "status": "not_found",
            "message": "The project record holds no payment schedule for the unit this call \
                        named.",
        })),
    }
}

fn milestone_statement(milestone: &Milestone, as_of: &str) -> String {
    match (milestone.status.as_str(), &milestone.actual_date) {
        ("complete", Some(date)) => format!(
            "Milestone {} ({}) is recorded complete on {date}.",
            milestone.id, milestone.title,
        ),
        ("in_progress", _) => match milestone.percent_complete {
            Some(percent) => format!(
                "Milestone {} ({}) is in progress at {percent}% as at {as_of}, planned for {}.",
                milestone.id, milestone.title, milestone.planned_date,
            ),
            None => format!(
                "Milestone {} ({}) is in progress as at {as_of}, planned for {}.",
                milestone.id, milestone.title, milestone.planned_date,
            ),
        },
        (status, _) => format!(
            "Milestone {} ({}) is recorded as {status}, planned for {}.",
            milestone.id, milestone.title, milestone.planned_date,
        ),
    }
}

/// A dated change in the record, with the evidence behind it.
struct Event {
    kind: &'static str,
    id: String,
    date: String,
    statement: String,
    /// The documents that evidence it and may be cited to this caller. Empty
    /// means this call could not state it: the change exists, and nothing
    /// here may speak for it.
    citations: Vec<String>,
    /// Whether this caller may read *any* document that evidences the change,
    /// whether or not it verified in this call.
    ///
    /// `false` with no citations and `true` with no citations are two
    /// different states and must not be collapsed. Evidence the caller may
    /// read that failed verification is still their change, owed to them, and
    /// is offered again when it verifies. Evidence they may not read at all
    /// is not their change in the first place: MS-10 is a public milestone
    /// whose only evidence is a developer-only certificate, so a buyer may
    /// never hear its date — not from `get_milestones`, not from
    /// `verify_claim`, and not by sending a cursor that names it and having
    /// it handed back.
    visible_evidence: bool,
}

impl Event {
    /// What a cursor records once this change has been stated to a caller.
    ///
    /// The date is part of it on purpose: a certificate re-issued on a later
    /// date is a new change, and a caller who was told about the old one has
    /// not been told about this one.
    fn change_id(&self) -> String {
        format!("{}:{}:{}", self.kind, self.id, self.date)
    }

    /// Whether this call may state it.
    fn statable(&self) -> bool {
        !self.citations.is_empty()
    }
}

/// Every dated event after `since` that this caller may hear, oldest first.
///
/// "Dated" means the record's own dates: a milestone's `actual_date` and a
/// certificate's `issued_date`. Events whose evidence did not verify in this
/// call are returned too, with no citations, because they are still changes
/// this caller has a claim to — they are simply not stateable now, and
/// [`whats_changed`] leaves them out of the cursor so they come back.
///
/// Events the caller may read no evidence for are returned as well, marked
/// `visible_evidence: false`, because a caller of [`dated_events`] has to be
/// able to tell the two apart. Nothing about them is ever stated or
/// confirmed.
fn dated_events(
    dataset: &VerifiedDataset,
    role: &Role,
    since: &str,
    index: &mut DocumentIndex<'_>,
) -> Vec<Event> {
    let record = &dataset.record;
    let mut events = Vec::new();

    for milestone in visible_milestones(record, role) {
        let Some(date) = milestone.actual_date.as_deref() else {
            continue;
        };
        if date <= since {
            continue;
        }
        let (evidence_ids, _note) = milestone_evidence(milestone, index);
        // The same test `get_milestones` withholds a milestone by, so the two
        // routes cannot disagree about whose change this is.
        let visible_evidence = evidence_ids.iter().any(|id| index.visible(id));
        let citations = index.cite(evidence_ids);
        events.push(Event {
            kind: "milestone",
            id: milestone.id.clone(),
            date: date.to_owned(),
            statement: milestone_statement(milestone, &record.project.as_of),
            citations,
            visible_evidence,
        });
    }

    for certificate in visible_certificates(record, role) {
        let Some(date) = certificate.issued_date.as_deref() else {
            continue;
        };
        if date <= since {
            continue;
        }
        let visible_evidence = index.visible(&certificate.document);
        let citations = index.cite([PROJECT_RECORD_DOC_ID, certificate.document.as_str()]);
        // The certificate document itself must verify: announcing the
        // certificate as news on the record alone would state what this call
        // cannot stand behind.
        let evidenced = citations.iter().any(|id| id == &certificate.document);
        events.push(Event {
            kind: "certificate",
            id: certificate.id.clone(),
            date: date.to_owned(),
            statement: certificate_statement(certificate, record, "verified"),
            citations: if evidenced { citations } else { Vec::new() },
            visible_evidence,
        });
    }

    events.sort_by(|left, right| left.date.cmp(&right.date).then(left.id.cmp(&right.id)));
    events
}

/// What the record says on one topic, answered only from facts that verified.
///
/// This is a **deterministic composer, not a language model and not a question
/// parser**: the caller names the topic, this states every fact the record
/// holds under it for their role with the citations behind each one, and says
/// plainly when it can state none. No network call and no model inference
/// happens here, and nothing is generated that is not a record field.
///
/// `Err` is the refusal a call earns by naming a unit the caller's token does
/// not cover — the same refusal `get_payment_status` gives, because asking a
/// question is not a way around a boundary.
pub fn ask_project(
    dataset: &VerifiedDataset,
    role: &Role,
    topic: Topic,
    unit: Option<&str>,
) -> Result<Value, Value> {
    // Scope first, for every topic: a unit outside the token is refused
    // whatever was asked about it, and before anything about it is read.
    let schedules = schedules_asked_about(&dataset.record, role, unit).map_err(|mut refusal| {
        refusal["answered"] = json!(false);
        refusal["answer"] = refusal["message"].clone();
        refusal["topic"] = json!(topic.as_str());
        refusal["role"] = json!(role.to_string());
        refusal
    })?;

    let mut index = DocumentIndex::new(dataset, role);
    let facts = facts_for(dataset, role, topic, &schedules, &mut index);

    let answer = if facts.is_empty() {
        format!(
            "I cannot verify that. The signed project record for {} states nothing under \
             {} that this call can evidence for your role.",
            dataset.record.project.name,
            topic.as_str(),
        )
    } else {
        let mut answer = String::new();
        for fact in &facts {
            if !answer.is_empty() {
                answer.push(' ');
            }
            answer.push_str(&fact.statement);
            answer.push_str(" [");
            answer.push_str(&fact.citations.join(", "));
            answer.push(']');
        }
        answer
    };

    Ok(json!({
        "topic": topic.as_str(),
        "answered": !facts.is_empty(),
        "answer": answer,
        "facts": facts.iter().map(Fact::render).collect::<Vec<Value>>(),
        // The units this answer actually spoke about. Only the payments topic
        // is per-unit; the rest are the project's, so they name none.
        "units": if topic == Topic::Payments {
            schedules.iter().map(|schedule| schedule.unit.clone()).collect()
        } else {
            Vec::<String>::new()
        },
        "role": role.to_string(),
        "documents": index.documents(),
        "unverified": index.unverified(),
        "verification": verification(dataset, role),
        "composer": "deterministic: the caller names a topic, this states the record's fields \
                     under it, no model and no network. Every sentence above is assembled from \
                     record fields and cites the documents that evidence it.",
    }))
}

/// Check one stated value against one named field of the signed record.
///
/// There is no sentence here to interpret and no default answer. The caller
/// names a subject and one of its fields; `SUPPORTED` means that field was
/// read and equals the value stated, `CONTRADICTED` means it was read and
/// differs. Everything else is `UNVERIFIED` with a `reason`: the subject is
/// outside the caller's scope or not in the record, the field is not one of
/// that subject's, the record states no such value, the stated value is not
/// one this server can read under that field's rule, or the evidence behind
/// the subject did not verify in this call.
///
/// A verified document is a *precondition* for judging, never a verdict.
///
/// Nothing the caller sent is echoed back except the two closed-enum values
/// this server itself declared. A `subject_id` is the caller's own string and
/// can carry an id they may not read; repeating it in a refusal would state it.
pub fn verify_claim(dataset: &VerifiedDataset, role: &Role, assertion: &Assertion<'_>) -> Value {
    let mut index = DocumentIndex::new(dataset, role);
    let outcome = judge(dataset, role, assertion, &mut index);

    json!({
        "subject_type": assertion.subject_type.as_str(),
        "field": assertion.field_name,
        "verdict": outcome.verdict,
        "reason": outcome.reason,
        "subject": outcome.subject,
        "finding": outcome.finding,
        "supporting_documents": index.documents(),
        "unverified": index.unverified(),
        "role": role.to_string(),
        "verification": verification(dataset, role),
    })
}

/// What `verify_claim` concluded, and why.
struct Judgement {
    verdict: &'static str,
    /// The machine-readable kind of `UNVERIFIED`; absent for a verdict that
    /// was reached by comparing a field.
    reason: Option<&'static str>,
    /// The entity that was judged, with the evidence behind it. Absent when
    /// none was resolved, or when the caller may not be told one was.
    subject: Option<Value>,
    finding: String,
}

impl Judgement {
    /// A refusal to judge, with the kind of refusal it is.
    fn unverified(reason: &'static str, finding: String) -> Self {
        Self {
            verdict: "UNVERIFIED",
            reason: Some(reason),
            subject: None,
            finding,
        }
    }
}

/// The subject an assertion resolved to, and the evidence behind it.
struct Subject {
    /// How it reads in the answer.
    rendered: Value,
    /// How it reads in a sentence: `unit 1204`, `milestone MS-07`.
    phrase: String,
    /// Whether any document that evidences it verified in this call.
    evidenced: bool,
}

fn judge(
    dataset: &VerifiedDataset,
    role: &Role,
    assertion: &Assertion<'_>,
    index: &mut DocumentIndex<'_>,
) -> Judgement {
    let record = &dataset.record;

    // Scope first, and before anything about that unit is looked up. A unit
    // this caller may not read is refused whatever the claim says about it.
    if assertion.subject_type == SubjectType::Unit
        && let Role::Buyer { unit: own } = role
        && own.as_str() != assertion.subject_id
    {
        return Judgement::unverified(
            "role_scope",
            format!(
                "This bearer token is bound to unit {own} and cannot read another unit's \
                 payment schedule, so this server cannot check that claim."
            ),
        );
    }

    let Some(field) = assertion.field else {
        return Judgement::unverified(
            "assertion_not_understood",
            format!(
                "`{}` is not a field of a {}. This server compares one named field of one \
                 subject, and that pairing names none.",
                assertion.field_name,
                assertion.subject_type.as_str(),
            ),
        );
    };

    if field.needs_instalment() && assertion.instalment_id.is_none() {
        return Judgement::unverified(
            "subject_not_resolved",
            format!(
                "`{}` is about one instalment of a schedule, and this call named none, so \
                 there was nothing to compare.",
                assertion.field_name,
            ),
        );
    }
    if !field.needs_instalment() && assertion.instalment_id.is_some() {
        return Judgement::unverified(
            "assertion_not_understood",
            format!(
                "`{}` is not about one instalment, but this call named one. This server does \
                 not choose between the two.",
                assertion.field_name,
            ),
        );
    }

    // Not in the field's canonical form. This is UNVERIFIED and never
    // CONTRADICTED, and the difference is the whole point: `IN_PROGRESS` for
    // a milestone the record records as `in_progress` is a *true* claim this
    // server did not read, and calling a true claim false is the one thing
    // this server exists not to do. The reason is its own word because
    // "I could not read your value" is a different fault from "that is not a
    // field of that subject", and a caller fixes them differently.
    let Some(stated) = field.kind().normalise(assertion.asserted_value) else {
        let vocabulary = field.kind().vocabulary().map_or_else(String::new, |words| {
            format!(
                " The words `{}` reads are {}.",
                assertion.field_name,
                words
                    .iter()
                    .map(|word| format!("`{word}`"))
                    .collect::<Vec<String>>()
                    .join(", "),
            )
        });
        return Judgement::unverified(
            "assertion_not_normalised",
            format!(
                "The value stated for `{}` is not in that field's canonical form, so this \
                 server did not read it and compared it with nothing — it is neither \
                 supported nor contradicted. The form is {}.{vocabulary}",
                assertion.field_name,
                field.kind().rule(),
            ),
        );
    };

    let (subject, recorded) = match assertion.subject_type {
        SubjectType::Milestone => {
            let Some(milestone) = visible_milestones(record, role)
                .into_iter()
                .find(|milestone| milestone.id == assertion.subject_id)
            else {
                return unresolved_subject(assertion.subject_type);
            };
            let (evidence_ids, _note) = milestone_evidence(milestone, index);
            let citations = index.cite(evidence_ids);
            (
                Subject {
                    rendered: json!({
                        "kind": "milestone",
                        "id": milestone.id,
                        "citations": citations.clone(),
                    }),
                    phrase: format!("milestone {}", milestone.id),
                    evidenced: !citations.is_empty(),
                },
                assertion::milestone_value(field, milestone),
            )
        }
        SubjectType::Certificate => {
            let Some(certificate) = visible_certificates(record, role)
                .into_iter()
                .find(|certificate| certificate.id == assertion.subject_id)
            else {
                return unresolved_subject(assertion.subject_type);
            };
            let citations = index.cite([PROJECT_RECORD_DOC_ID, certificate.document.as_str()]);
            let evidenced = citations.iter().any(|id| id == &certificate.document);
            (
                Subject {
                    rendered: json!({
                        "kind": "certificate",
                        "id": certificate.id,
                        "citations": citations,
                    }),
                    phrase: format!("certificate {}", certificate.id),
                    evidenced,
                },
                assertion::certificate_value(
                    field,
                    certificate,
                    &record.issuer_name(&certificate.issuer),
                ),
            )
        }
        SubjectType::Unit => {
            let Some(schedule) = record.schedule_for(assertion.subject_id) else {
                return unresolved_subject(assertion.subject_type);
            };
            let citations = index.cite([PROJECT_RECORD_DOC_ID, schedule.document.as_str()]);
            let evidenced = citations.iter().any(|id| id == &schedule.document);
            let (phrase, recorded) = match assertion.instalment_id {
                None => (
                    format!("unit {}", schedule.unit),
                    assertion::unit_value(field, schedule),
                ),
                Some(instalment_id) => {
                    let Some(instalment) = schedule
                        .instalments
                        .iter()
                        .find(|instalment| instalment.id == instalment_id)
                    else {
                        return Judgement::unverified(
                            "subject_not_resolved",
                            format!(
                                "The schedule for unit {} holds no instalment with the id this \
                                 call named, so there was nothing to compare.",
                                schedule.unit,
                            ),
                        );
                    };
                    (
                        format!("instalment {} of unit {}", instalment.id, schedule.unit),
                        assertion::instalment_value(field, instalment),
                    )
                }
            };
            (
                Subject {
                    rendered: json!({
                        "kind": "unit",
                        "id": schedule.unit,
                        "instalment": assertion.instalment_id,
                        "citations": citations,
                    }),
                    phrase,
                    evidenced,
                },
                recorded,
            )
        }
    };

    conclude(assertion.field_name, &subject, recorded.as_ref(), &stated)
}

/// Compare one stated value with the record's own, once everything else has
/// been resolved.
fn conclude(
    field_name: &str,
    subject: &Subject,
    recorded: Option<&Reading>,
    stated: &Reading,
) -> Judgement {
    if !subject.evidenced {
        return Judgement {
            verdict: "UNVERIFIED",
            reason: Some("evidence_unverified"),
            subject: Some(subject.rendered.clone()),
            finding: format!(
                "No source document available in this call evidences {}, so this server does \
                 not state what the record says about it.",
                subject.phrase,
            ),
        };
    }
    let Some(recorded) = recorded else {
        return Judgement {
            verdict: "UNVERIFIED",
            reason: Some("not_stated_by_record"),
            subject: Some(subject.rendered.clone()),
            finding: format!(
                "The signed record states no `{field_name}` for {}, so there was nothing to \
                 compare.",
                subject.phrase,
            ),
        };
    };
    Judgement {
        verdict: if recorded == stated {
            "SUPPORTED"
        } else {
            "CONTRADICTED"
        },
        reason: None,
        subject: Some(subject.rendered.clone()),
        finding: format!(
            "The signed record states `{field_name}` for {} as `{}`.",
            subject.phrase,
            recorded.display(),
        ),
    }
}

/// Nothing in this caller's view of the record answers to the id that was
/// named. The id itself is not repeated: for a buyer it may be a document they
/// may not read, and this refusal is the same whether the subject is invisible
/// to them or absent from the record entirely.
fn unresolved_subject(subject_type: SubjectType) -> Judgement {
    Judgement::unverified(
        "subject_not_resolved",
        format!(
            "This caller's view of the signed record holds no {} with the id this call named, \
             so there was nothing to compare.",
            subject_type.as_str(),
        ),
    )
}

/// What changed after the point the caller's cursor marks.
///
/// The answer carries a `next_cursor`, and that is the only record of delivery
/// there is: this server writes nothing down. A change this call could not
/// state — because its evidence did not verify — is not in the returned
/// cursor, so it is offered again rather than being silently skipped.
///
/// The incoming cursor is never trusted on its own, and the rule is narrower
/// than "it matched a change". An id survives into `next_cursor` only when
/// this call found a change with that id **and** this caller may read at
/// least one document that evidences it. So the only ids a cursor can ever
/// carry back are ids of changes this same call could have stated to this
/// same caller: a cursor can shrink to the changes the caller may hear and
/// can never grow, and it can never be used to have a date confirmed.
/// Sending another caller's cursor, or sixty-two guessed ids, tells the
/// sender exactly nothing they did not already have.
pub fn whats_changed(dataset: &VerifiedDataset, role: &Role, carried: &BTreeSet<String>) -> Value {
    let mut index = DocumentIndex::new(dataset, role);
    let events = dated_events(dataset, role, CHANGE_BASELINE_DATE, &mut index);

    let mut next = BTreeSet::new();
    let mut changes = Vec::new();
    let mut carried_forward = 0usize;
    for event in &events {
        if !event.visible_evidence {
            // Not this caller's change to hear at all: no document they may
            // read evidences it. Neither stated nor confirmed — echoing the
            // id back would tell them the change, and its date, exist.
            continue;
        }
        let change_id = event.change_id();
        if carried.contains(&change_id) {
            // Already delivered, and still a change this caller may hear: it
            // stays in the cursor even while its evidence is failing, so a
            // document that breaks after delivery does not re-announce it.
            carried_forward += 1;
            next.insert(change_id);
            continue;
        }
        if !event.statable() {
            continue;
        }
        changes.push(json!({
            "kind": event.kind,
            "id": event.id,
            "date": event.date,
            "change_id": change_id,
            "statement": event.statement,
            "citations": event.citations,
        }));
        next.insert(change_id);
    }

    let message = if changes.is_empty() {
        format!(
            "Nothing in the signed record dated after {CHANGE_BASELINE_DATE} is still \
             undelivered to you: your cursor already accounts for every change this server can \
             state."
        )
    } else {
        format!(
            "{} change(s) in the signed record, dated after {CHANGE_BASELINE_DATE}, that your \
             cursor does not account for.",
            changes.len(),
        )
    };

    json!({
        "project": dataset.record.project.name,
        "role": role.to_string(),
        "baseline": CHANGE_BASELINE_DATE,
        "changes": changes,
        "next_cursor": cursor::encode(&next),
        "message": message,
        "delivery": {
            "carried_forward": carried_forward,
            "state": "none on this server: what you have been told is carried by the cursor in \
                      this answer, so a call whose answer never reached you cost you nothing",
            "kept": "a change this call could not state is not in next_cursor, so it is offered \
                     again once its evidence verifies",
            "cursor": "opaque: send it back unchanged as `since_cursor`, and do not compose one",
        },
        "documents": index.documents(),
        "unverified": index.unverified(),
        "verification": verification(dataset, role),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_topic_is_one_of_five_words() {
        for topic in Topic::ALL {
            assert_eq!(Topic::parse(topic.as_str()), Some(topic));
        }
        for refused in ["handover", "changes", "Overview", "", "payments "] {
            assert_eq!(
                Topic::parse(refused),
                None,
                "`{refused}` is not a topic this server states",
            );
        }
    }

    #[test]
    fn a_change_id_names_the_kind_the_entity_and_the_date() {
        let event = Event {
            kind: "certificate",
            id: "CERT-ESCROW-2026-209".to_owned(),
            date: "2026-10-05".to_owned(),
            statement: String::new(),
            citations: vec!["CERT-ESCROW-2026-209".to_owned()],
            visible_evidence: true,
        };
        assert_eq!(
            event.change_id(),
            "certificate:CERT-ESCROW-2026-209:2026-10-05",
        );
        assert!(event.statable());

        let unevidenced = Event {
            citations: Vec::new(),
            ..event
        };
        assert!(
            !unevidenced.statable(),
            "a change with no citation is a change this call cannot state",
        );
    }
}
