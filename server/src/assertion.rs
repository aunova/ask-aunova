//! The typed assertion `verify_claim` checks, and the one normalisation each
//! field type is allowed.
//!
//! **Nothing in this module reads English.** It holds no tokeniser, no
//! keyword list, no negation rule and no sentence shapes. A caller — in the
//! demo, the model that turned a person's sentence into arguments — names a
//! subject, names one field of that subject, and states what it believes that
//! field to be. This module turns the stated value into one of five typed
//! readings and compares it with the record's own value for that field.
//!
//! Four rules hold, and together they are the whole of what "comparison" means
//! here.
//!
//! 1. **One assertion names exactly one field.** There is no sentence to
//!    account for, so there is nothing left over to be silently ignored.
//! 2. **Normalisation is per field type, documented, and narrow enough that it
//!    cannot be creative.** [`FieldKind::rule`] is the single statement of each
//!    rule; it is sent to callers in the tool schema and quoted back in the
//!    refusal when a value does not meet it, so the contract and the error
//!    cannot drift apart.
//! 3. **Money is an integer in minor units and nothing else.** Every integer
//!    is read as minor units, because nothing distinguishes one written in
//!    whole dirhams from one written in fils: `1,470,000.00 AED` is stated as
//!    `147000000`, and `1470000` is a well-formed statement of 14,700.00 AED
//!    which this server compares and finds different. There is no scale to
//!    infer and none is inferred, so a currency code or symbol, a thousands
//!    separator, a decimal point and a scale suffix are refused rather than
//!    guessed at: `1.47m` and `1,470,000.00 AED` are read as nothing.
//! 4. **Comparison is equality of the normalised values.** Equal is
//!    `SUPPORTED`, unequal is `CONTRADICTED`, and "the record states no such
//!    value" is neither.
//!
//! What is deliberately *not* here: `visibility`, `evidence`, `contractor_note`
//! and a certificate's or schedule's `document` are fields of `project.json`
//! that no tool states, and they are not checkable. Letting a buyer ask
//! "is `CN-14` the contractor note behind `MS-03`?" would answer a question
//! about an internal document by refusing it in a distinguishable way, which
//! is the leak this server exists to avoid. Every field below is one some tool
//! already states to a caller who may hear it.

use crate::record::{Certificate, Instalment, Milestone, PaymentSchedule};

/// Which part of the record an assertion is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectType {
    /// One unit's payment schedule, and optionally one instalment of it.
    Unit,
    /// One certificate.
    Certificate,
    /// One construction milestone.
    Milestone,
}

impl SubjectType {
    /// Every subject type, in the order the tool schema lists them.
    pub const ALL: [SubjectType; 3] = [
        SubjectType::Unit,
        SubjectType::Certificate,
        SubjectType::Milestone,
    ];

    /// The word a caller sends.
    pub const fn as_str(self) -> &'static str {
        match self {
            SubjectType::Unit => "unit",
            SubjectType::Certificate => "certificate",
            SubjectType::Milestone => "milestone",
        }
    }

    /// Read a caller's `subject_type`. `None` is a value outside the enum.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// What `subject_id` names for this subject type, for the tool schema.
    pub const fn id_description(self) -> &'static str {
        match self {
            SubjectType::Unit => {
                "Unit number exactly as printed in the sale agreement, for example `1204`."
            }
            SubjectType::Certificate => "Certificate id, for example `CERT-FIRE-2026-087`.",
            SubjectType::Milestone => "Milestone id, for example `MS-07`.",
        }
    }

    /// The fields that may be checked on this subject type.
    pub const fn fields(self) -> &'static [Field] {
        match self {
            SubjectType::Unit => &[
                Field::TotalPriceMinor,
                Field::PaidToDateMinor,
                Field::OutstandingMinor,
                Field::HasOverdueInstalment,
                Field::EscrowAccountRef,
                Field::UnitType,
                Field::InstalmentStatus,
                Field::InstalmentAmountMinor,
                Field::InstalmentPercent,
                Field::InstalmentDueDate,
                Field::InstalmentPaidDate,
                Field::InstalmentLabel,
                Field::InstalmentTrigger,
            ],
            SubjectType::Certificate => &[
                Field::CertificateStatus,
                Field::CertificateTitle,
                Field::CertificateIssuer,
                Field::CertificateIssuerId,
                Field::CertificateIssuedDate,
                Field::CertificateExpiresDate,
                Field::CertificateScope,
            ],
            SubjectType::Milestone => &[
                Field::MilestoneStatus,
                Field::MilestoneTitle,
                Field::MilestonePlannedDate,
                Field::MilestoneActualDate,
                Field::MilestonePercentComplete,
                Field::MilestonePercentOfWorks,
            ],
        }
    }
}

/// One checkable field of one subject.
///
/// Two subject types spell two different fields the same way — a certificate's
/// `status` is not a milestone's — so a field name is only ever resolved
/// against a subject type, never on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Unit: the contract price, in minor units.
    TotalPriceMinor,
    /// Unit: paid so far, in minor units.
    PaidToDateMinor,
    /// Unit: contract price less what has been paid, in minor units.
    OutstandingMinor,
    /// Unit: whether any instalment of the schedule is recorded overdue.
    HasOverdueInstalment,
    /// Unit: the escrow account the schedule names.
    EscrowAccountRef,
    /// Unit: the unit's description in the record.
    UnitType,
    /// Instalment: its recorded status.
    InstalmentStatus,
    /// Instalment: its amount, in minor units.
    InstalmentAmountMinor,
    /// Instalment: its share of the contract price, in whole percent.
    InstalmentPercent,
    /// Instalment: the date it falls due.
    InstalmentDueDate,
    /// Instalment: the date it was paid.
    InstalmentPaidDate,
    /// Instalment: what the schedule calls it.
    InstalmentLabel,
    /// Instalment: the milestone (or `contract`) that triggers it.
    InstalmentTrigger,
    /// Certificate: `issued`, `pending`, or whatever the record records.
    CertificateStatus,
    /// Certificate: what it attests.
    CertificateTitle,
    /// Certificate: the issuer's display name.
    CertificateIssuer,
    /// Certificate: the issuer's id.
    CertificateIssuerId,
    /// Certificate: date of issue.
    CertificateIssuedDate,
    /// Certificate: expiry date.
    CertificateExpiresDate,
    /// Certificate: the scope of the attestation.
    CertificateScope,
    /// Milestone: `complete`, `in_progress` or `not_started`.
    MilestoneStatus,
    /// Milestone: what it covers.
    MilestoneTitle,
    /// Milestone: the programme date.
    MilestonePlannedDate,
    /// Milestone: the date it actually completed.
    MilestoneActualDate,
    /// Milestone: progress within an in-progress milestone, in whole percent.
    MilestonePercentComplete,
    /// Milestone: share of the whole works, in whole percent.
    MilestonePercentOfWorks,
}

impl Field {
    /// The name a caller sends.
    ///
    /// Two names repeat across subject types on purpose: a caller says
    /// `status` whatever the subject is, and the subject type decides which
    /// `status` that is.
    pub const fn as_str(self) -> &'static str {
        match self {
            Field::TotalPriceMinor => "total_price_minor",
            Field::PaidToDateMinor => "paid_to_date_minor",
            Field::OutstandingMinor => "outstanding_minor",
            Field::HasOverdueInstalment => "has_overdue_instalment",
            Field::EscrowAccountRef => "escrow_account_ref",
            Field::UnitType => "unit_type",
            Field::InstalmentStatus => "instalment_status",
            Field::InstalmentAmountMinor => "instalment_amount_minor",
            Field::InstalmentPercent => "instalment_percent",
            Field::InstalmentDueDate => "instalment_due_date",
            Field::InstalmentPaidDate => "instalment_paid_date",
            Field::InstalmentLabel => "instalment_label",
            Field::InstalmentTrigger => "instalment_trigger",
            Field::CertificateStatus | Field::MilestoneStatus => "status",
            Field::CertificateTitle | Field::MilestoneTitle => "title",
            Field::CertificateIssuer => "issuer",
            Field::CertificateIssuerId => "issuer_id",
            Field::CertificateIssuedDate => "issued_date",
            Field::CertificateExpiresDate => "expires_date",
            Field::CertificateScope => "scope",
            Field::MilestonePlannedDate => "planned_date",
            Field::MilestoneActualDate => "actual_date",
            Field::MilestonePercentComplete => "percent_complete",
            Field::MilestonePercentOfWorks => "percent_of_works",
        }
    }

    /// How a stated value for this field is read.
    pub const fn kind(self) -> FieldKind {
        match self {
            Field::TotalPriceMinor
            | Field::PaidToDateMinor
            | Field::OutstandingMinor
            | Field::InstalmentAmountMinor => FieldKind::Money,
            Field::HasOverdueInstalment => FieldKind::Boolean,
            Field::InstalmentPercent
            | Field::MilestonePercentComplete
            | Field::MilestonePercentOfWorks => FieldKind::Percent,
            Field::InstalmentDueDate
            | Field::InstalmentPaidDate
            | Field::CertificateIssuedDate
            | Field::CertificateExpiresDate
            | Field::MilestonePlannedDate
            | Field::MilestoneActualDate => FieldKind::Date,
            Field::InstalmentStatus => FieldKind::Vocabulary(INSTALMENT_STATUSES),
            Field::CertificateStatus => FieldKind::Vocabulary(CERTIFICATE_STATUSES),
            Field::MilestoneStatus => FieldKind::Vocabulary(MILESTONE_STATUSES),
            Field::EscrowAccountRef
            | Field::UnitType
            | Field::InstalmentLabel
            | Field::InstalmentTrigger
            | Field::CertificateTitle
            | Field::CertificateIssuer
            | Field::CertificateIssuerId
            | Field::CertificateScope
            | Field::MilestoneTitle => FieldKind::Text,
        }
    }

    /// Whether this field is about one instalment rather than the schedule.
    pub const fn needs_instalment(self) -> bool {
        matches!(
            self,
            Field::InstalmentStatus
                | Field::InstalmentAmountMinor
                | Field::InstalmentPercent
                | Field::InstalmentDueDate
                | Field::InstalmentPaidDate
                | Field::InstalmentLabel
                | Field::InstalmentTrigger
        )
    }

    /// The field of `subject_type` that a caller's name refers to.
    ///
    /// `None` means the name is not a field of *this* subject — a certificate
    /// has no `paid_to_date_minor` — which is a claim this server cannot
    /// check, not a claim it can call false.
    pub fn parse(subject_type: SubjectType, name: &str) -> Option<Self> {
        subject_type
            .fields()
            .iter()
            .copied()
            .find(|field| field.as_str() == name)
    }

    /// Every field name a caller may send, deduplicated, for the tool schema's
    /// closed enum. A name outside this list fails at the schema.
    pub fn declared_names() -> Vec<&'static str> {
        let mut names = Vec::new();
        for subject_type in SubjectType::ALL {
            for field in subject_type.fields() {
                let name = field.as_str();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }
}

/// The words a milestone's `status` may take, in the record's own spelling.
///
/// These three lists are the *only* statement of each vocabulary. The tool
/// schema declares them as an `enum` and this module validates against them,
/// both from here, so a caller cannot be shown one spelling and judged by
/// another. A status the record states that is not here would never be
/// matched by the value the record itself holds, which is why
/// `every_status_the_record_states_is_in_its_field_vocabulary` checks the
/// committed record against them.
const MILESTONE_STATUSES: &[&str] = &["complete", "in_progress", "not_started"];
/// The words a certificate's `status` may take.
const CERTIFICATE_STATUSES: &[&str] = &["issued", "pending"];
/// The words an instalment's `instalment_status` may take.
const INSTALMENT_STATUSES: &[&str] = &["scheduled", "paid", "paid_late", "overdue"];

/// How a stated value is read, and the one rule that reading allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// An exact integer count of minor units.
    Money,
    /// An ISO-8601 calendar date.
    Date,
    /// A whole percentage.
    Percent,
    /// `true` or `false`.
    Boolean,
    /// One word of a closed vocabulary, carried here so the schema's `enum`
    /// and the check this server runs are the same list.
    Vocabulary(&'static [&'static str]),
    /// Free text, compared exactly.
    Text,
}

impl FieldKind {
    /// The rule a stated value of this kind must meet, in the words sent to
    /// callers in the tool schema and quoted back when a value fails it.
    ///
    /// Each one shows the canonical form rather than only naming it: one
    /// worked conversion stops more wrong calls than three sentences of rule.
    pub const fn rule(self) -> &'static str {
        match self {
            FieldKind::Money => {
                "an exact integer count of minor units (fils), optionally signed, and nothing \
                 else — 1,470,000.00 AED is written `147000000`. Every integer is read as \
                 minor units, so `1470000` states 14,700.00 AED and is compared as that; \
                 there is no scale to infer and none is inferred. A currency code or symbol, \
                 a thousands separator, a decimal point and a scale suffix are refused rather \
                 than guessed at, so `1,470,000.00 AED` and `1.47m` are read as nothing"
            }
            FieldKind::Date => {
                "an ISO-8601 calendar date, exactly `YYYY-MM-DD` — 4 September 2026 is written \
                 `2026-09-04`: ten ASCII characters, digits with a hyphen after the year and \
                 after the month, so `2026-9-4` and `04/09/2026` are read as nothing"
            }
            FieldKind::Percent => {
                "a whole number from 0 to 100 and nothing else — 48 per cent is written `48`, \
                 so `48%` and `48.0` are read as nothing"
            }
            FieldKind::Boolean => "exactly `true` or `false`, lowercase — not `True`, not `yes`",
            FieldKind::Vocabulary(_) => {
                "one word of that field's own closed vocabulary, matched exactly and \
                 case-sensitively — `in_progress`, never `In_progress`, `IN_PROGRESS`, \
                 `in progress` or `in-progress`. The words are listed per `subject_type` and \
                 `field` in this schema's `allOf`, and a value outside them is read as nothing \
                 rather than compared"
            }
            FieldKind::Text => {
                "the text exactly as the record writes it, matched character for character: no \
                 substring match and no fuzzy match"
            }
        }
    }

    /// The closed vocabulary this kind reads, when it reads one.
    ///
    /// The tool schema narrows `asserted_value` to exactly this list, so a
    /// caller can discover the spelling instead of guessing at it.
    pub const fn vocabulary(self) -> Option<&'static [&'static str]> {
        match self {
            FieldKind::Vocabulary(words) => Some(words),
            FieldKind::Money
            | FieldKind::Date
            | FieldKind::Percent
            | FieldKind::Boolean
            | FieldKind::Text => None,
        }
    }

    /// Read a stated value under this kind's rule.
    ///
    /// The value arrives already trimmed of surrounding whitespace — the one
    /// step applied before any rule here, so that a stray space is not read as
    /// a different value — and nothing else is done to it.
    ///
    /// `None` is "this server did not read your value", and it is never a
    /// verdict. A status spelled any way but the record's own is one of
    /// these: the record says `in_progress`, and `IN_PROGRESS` is a claim
    /// about the same milestone that this server will not pretend to have
    /// understood — calling it false would be calling a true claim false.
    pub fn normalise(self, value: &str) -> Option<Reading> {
        match self {
            FieldKind::Money => parse_minor_units(value).map(Reading::Money),
            FieldKind::Date => parse_iso_date(value).map(Reading::Date),
            FieldKind::Percent => parse_percent(value).map(Reading::Percent),
            FieldKind::Boolean => match value {
                "true" => Some(Reading::Boolean(true)),
                "false" => Some(Reading::Boolean(false)),
                _ => None,
            },
            FieldKind::Vocabulary(words) => words
                .iter()
                .copied()
                .find(|word| *word == value)
                .map(|word| Reading::Text(word.to_owned())),
            FieldKind::Text => (!value.is_empty()).then(|| Reading::Text(value.to_owned())),
        }
    }
}

/// A value, normalised. Two readings are equal when the values they stand for
/// are the same, which is the whole of what a verdict rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// Minor units, exactly.
    Money(i64),
    /// `YYYY-MM-DD`.
    Date(String),
    /// Whole percent, 0 to 100.
    Percent(u32),
    /// A flag.
    Boolean(bool),
    /// Text, exactly as written.
    Text(String),
}

impl Reading {
    /// How the value reads in a finding, in the record's own characters.
    pub fn display(&self) -> String {
        match self {
            Reading::Money(amount) => amount.to_string(),
            Reading::Date(date) => date.clone(),
            Reading::Percent(percent) => percent.to_string(),
            Reading::Boolean(flag) => flag.to_string(),
            Reading::Text(text) => text.clone(),
        }
    }
}

/// An exact integer of minor units: an optional sign, then ASCII digits.
fn parse_minor_units(value: &str) -> Option<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // `parse` rejects anything that does not fit, so an amount larger than the
    // record can hold is refused rather than wrapped into a different number.
    value.parse::<i64>().ok()
}

/// `YYYY-MM-DD`, checked character by character.
///
/// The calendar is not checked beyond the shape: `2026-13-45` reads as a date
/// that no field of the record holds, so it compares unequal and the claim is
/// false — which is the right answer, and needs no month-length table to
/// reach.
fn parse_iso_date(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 {
        return None;
    }
    let shaped = bytes.iter().enumerate().all(|(index, byte)| match index {
        4 | 7 => *byte == b'-',
        _ => byte.is_ascii_digit(),
    });
    shaped.then(|| value.to_owned())
}

/// A whole percentage, 0 to 100.
fn parse_percent(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok().filter(|percent| *percent <= 100)
}

/// One typed assertion, as it arrives from a caller.
///
/// The strings borrow the request's own arguments: an assertion lives for one
/// call and is never stored, so there is nothing here to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assertion<'a> {
    /// Which part of the record the claim is about.
    pub subject_type: SubjectType,
    /// The id of the subject, as the caller wrote it.
    pub subject_id: &'a str,
    /// The field name as declared by this server, not the caller's copy of it.
    pub field_name: &'static str,
    /// The field that name resolves to on this subject type. `None` is a
    /// declared name that belongs to another subject type — an assertion this
    /// server cannot check, which is not the same as one it can call false.
    pub field: Option<Field>,
    /// What the caller says that field is, trimmed and not yet normalised.
    pub asserted_value: &'a str,
    /// The instalment named, when one was.
    pub instalment_id: Option<&'a str>,
}

impl<'a> Assertion<'a> {
    /// Build an assertion, resolving the field name against the subject type.
    ///
    /// `field_name` is one of [`Field::declared_names`], checked against that
    /// closed set before this is called.
    pub fn new(
        subject_type: SubjectType,
        subject_id: &'a str,
        field_name: &'static str,
        asserted_value: &'a str,
        instalment_id: Option<&'a str>,
    ) -> Self {
        Self {
            subject_type,
            subject_id,
            field_name,
            field: Field::parse(subject_type, field_name),
            asserted_value,
            instalment_id,
        }
    }
}

/// The record's own value for one field of a milestone.
///
/// `None` is "the record states no such value", which is not a comparison and
/// never a verdict.
pub fn milestone_value(field: Field, milestone: &Milestone) -> Option<Reading> {
    match field {
        Field::MilestoneStatus => Some(Reading::Text(milestone.status.clone())),
        Field::MilestoneTitle => Some(Reading::Text(milestone.title.clone())),
        Field::MilestonePlannedDate => Some(Reading::Date(milestone.planned_date.clone())),
        Field::MilestoneActualDate => milestone.actual_date.clone().map(Reading::Date),
        Field::MilestonePercentComplete => milestone.percent_complete.map(Reading::Percent),
        Field::MilestonePercentOfWorks => Some(Reading::Percent(milestone.percent_of_works)),
        _ => None,
    }
}

/// The record's own value for one field of a certificate.
///
/// `issuer_name` is the display name the record gives the issuer id, resolved
/// by the caller because the mapping lives on the record itself.
pub fn certificate_value(
    field: Field,
    certificate: &Certificate,
    issuer_name: &str,
) -> Option<Reading> {
    match field {
        Field::CertificateStatus => Some(Reading::Text(certificate.status.clone())),
        Field::CertificateTitle => Some(Reading::Text(certificate.title.clone())),
        Field::CertificateIssuer => Some(Reading::Text(issuer_name.to_owned())),
        Field::CertificateIssuerId => Some(Reading::Text(certificate.issuer.clone())),
        Field::CertificateIssuedDate => certificate.issued_date.clone().map(Reading::Date),
        Field::CertificateExpiresDate => certificate.expires_date.clone().map(Reading::Date),
        Field::CertificateScope => Some(Reading::Text(certificate.scope.clone())),
        _ => None,
    }
}

/// The record's own value for one schedule-level field of a unit.
///
/// Two of these are derived rather than stored, by the same integer arithmetic
/// `get_payment_status` states them with: `outstanding_minor` is the contract
/// price less what has been paid, and `has_overdue_instalment` is true when
/// any instalment of this schedule is recorded `overdue`. Neither discloses
/// anything the same answer does not already carry.
pub fn unit_value(field: Field, schedule: &PaymentSchedule) -> Option<Reading> {
    match field {
        Field::TotalPriceMinor => Some(Reading::Money(schedule.total_price_minor)),
        Field::PaidToDateMinor => Some(Reading::Money(schedule.paid_to_date_minor)),
        Field::OutstandingMinor => outstanding_minor(schedule).map(Reading::Money),
        Field::HasOverdueInstalment => Some(Reading::Boolean(has_overdue(schedule))),
        Field::EscrowAccountRef => Some(Reading::Text(schedule.escrow_account_ref.clone())),
        Field::UnitType => Some(Reading::Text(schedule.unit_type.clone())),
        _ => None,
    }
}

/// The record's own value for one field of one instalment.
pub fn instalment_value(field: Field, instalment: &Instalment) -> Option<Reading> {
    match field {
        Field::InstalmentStatus => Some(Reading::Text(instalment.status.clone())),
        Field::InstalmentAmountMinor => Some(Reading::Money(instalment.amount_minor)),
        Field::InstalmentPercent => Some(Reading::Percent(instalment.percent)),
        Field::InstalmentDueDate => Some(Reading::Date(instalment.due_date.clone())),
        Field::InstalmentPaidDate => instalment.paid_date.clone().map(Reading::Date),
        Field::InstalmentLabel => Some(Reading::Text(instalment.label.clone())),
        Field::InstalmentTrigger => Some(Reading::Text(instalment.trigger.clone())),
        _ => None,
    }
}

/// Contract price less what has been paid, in minor units.
///
/// Checked, so a record whose arithmetic does not fit an `i64` states nothing
/// here rather than stating a wrapped figure.
pub fn outstanding_minor(schedule: &PaymentSchedule) -> Option<i64> {
    schedule
        .total_price_minor
        .checked_sub(schedule.paid_to_date_minor)
}

/// Whether any instalment of this schedule is recorded overdue.
pub fn has_overdue(schedule: &PaymentSchedule) -> bool {
    schedule
        .instalments
        .iter()
        .any(|instalment| instalment.status == "overdue")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_is_minor_units_and_nothing_else() {
        assert_eq!(
            FieldKind::Money.normalise("147000000"),
            Some(Reading::Money(147_000_000)),
        );
        assert_eq!(FieldKind::Money.normalise("0"), Some(Reading::Money(0)));
        assert_eq!(FieldKind::Money.normalise("-1"), Some(Reading::Money(-1)));
        for refused in [
            "1,470,000.00 AED",
            "1470000.00",
            "1.47m",
            "1 470 000",
            "147000000 AED",
            "AED 147000000",
            "١٤٧",
            "+147000000",
            "",
            "-",
            "0x8c",
            "99999999999999999999",
        ] {
            assert_eq!(
                FieldKind::Money.normalise(refused),
                None,
                "`{refused}` must not read as an amount",
            );
        }
    }

    #[test]
    fn a_date_is_ten_characters_of_iso_8601() {
        assert_eq!(
            FieldKind::Date.normalise("2026-09-04"),
            Some(Reading::Date("2026-09-04".to_owned())),
        );
        for refused in [
            "2026-9-4",
            "04/09/2026",
            "2026-09-04T00:00:00Z",
            "September 4 2026",
            "2026-09-04 ",
            "",
        ] {
            assert_eq!(
                FieldKind::Date.normalise(refused),
                None,
                "`{refused}` must not read as a date",
            );
        }
    }

    #[test]
    fn a_percentage_is_a_whole_number_within_range() {
        assert_eq!(
            FieldKind::Percent.normalise("48"),
            Some(Reading::Percent(48))
        );
        assert_eq!(FieldKind::Percent.normalise("0"), Some(Reading::Percent(0)));
        assert_eq!(
            FieldKind::Percent.normalise("100"),
            Some(Reading::Percent(100))
        );
        for refused in ["48%", "48.0", "101", "-1", "forty-eight", ""] {
            assert_eq!(
                FieldKind::Percent.normalise(refused),
                None,
                "`{refused}` must not read as a percentage",
            );
        }
    }

    #[test]
    fn a_flag_is_one_of_two_lowercase_words() {
        assert_eq!(
            FieldKind::Boolean.normalise("true"),
            Some(Reading::Boolean(true)),
        );
        assert_eq!(
            FieldKind::Boolean.normalise("false"),
            Some(Reading::Boolean(false)),
        );
        for refused in ["True", "yes", "1", "TRUE", ""] {
            assert_eq!(
                FieldKind::Boolean.normalise(refused),
                None,
                "`{refused}` must not read as a flag",
            );
        }
    }

    #[test]
    fn text_is_compared_character_for_character() {
        assert_eq!(
            FieldKind::Text.normalise("Northwind Fire Safety Institute"),
            Some(Reading::Text("Northwind Fire Safety Institute".to_owned())),
        );
        assert_ne!(
            FieldKind::Text.normalise("northwind fire safety institute"),
            FieldKind::Text.normalise("Northwind Fire Safety Institute"),
            "case is part of the value",
        );
        assert_eq!(FieldKind::Text.normalise(""), None);
    }

    #[test]
    fn a_field_name_only_resolves_against_its_own_subject_type() {
        assert_eq!(
            Field::parse(SubjectType::Milestone, "status"),
            Some(Field::MilestoneStatus),
        );
        assert_eq!(
            Field::parse(SubjectType::Certificate, "status"),
            Some(Field::CertificateStatus),
        );
        assert_eq!(
            Field::parse(SubjectType::Certificate, "paid_to_date_minor"),
            None,
            "a certificate has no payment field, so that assertion is not checkable",
        );
        assert_eq!(Field::parse(SubjectType::Unit, "percent_complete"), None);
        assert_eq!(Field::parse(SubjectType::Unit, "nonsense"), None);
    }

    #[test]
    fn every_declared_name_resolves_on_at_least_one_subject_type() {
        let names = Field::declared_names();
        assert_eq!(
            names.len(),
            24,
            "the closed enum sent to callers changed: {names:?}",
        );
        for name in &names {
            assert!(
                SubjectType::ALL
                    .into_iter()
                    .any(|subject_type| Field::parse(subject_type, name).is_some()),
                "`{name}` is advertised but belongs to no subject type",
            );
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "a name is advertised twice");
    }

    #[test]
    fn only_instalment_fields_ask_for_an_instalment() {
        for field in SubjectType::Unit.fields() {
            assert_eq!(
                field.needs_instalment(),
                field.as_str().starts_with("instalment_"),
                "`{}` disagrees with its own name about needing an instalment",
                field.as_str(),
            );
        }
        for subject_type in [SubjectType::Certificate, SubjectType::Milestone] {
            for field in subject_type.fields() {
                assert!(!field.needs_instalment(), "{field:?}");
            }
        }
    }

    #[test]
    fn a_subject_type_is_one_of_three_words() {
        assert_eq!(SubjectType::parse("unit"), Some(SubjectType::Unit));
        assert_eq!(
            SubjectType::parse("certificate"),
            Some(SubjectType::Certificate)
        );
        assert_eq!(
            SubjectType::parse("milestone"),
            Some(SubjectType::Milestone)
        );
        for refused in ["Unit", "units", "escrow", ""] {
            assert_eq!(SubjectType::parse(refused), None, "`{refused}`");
        }
    }

    /// The class the r3 review found: a true claim spelled any way but the
    /// record's own came back CONTRADICTED. A value outside the vocabulary is
    /// not read at all, so there is nothing to call false.
    #[test]
    fn a_status_is_one_word_of_its_own_closed_vocabulary() {
        let milestone = Field::MilestoneStatus.kind();
        assert_eq!(
            milestone.normalise("in_progress"),
            Some(Reading::Text("in_progress".to_owned())),
        );
        for refused in [
            "In_progress",
            "IN_PROGRESS",
            "in progress",
            "in-progress",
            "completed",
            "issued",
            "paid",
            "",
        ] {
            assert_eq!(
                milestone.normalise(refused),
                None,
                "`{refused}` is not a milestone status, so it must not be read as one",
            );
        }

        assert_eq!(
            Field::InstalmentStatus.kind().normalise("paid_late"),
            Some(Reading::Text("paid_late".to_owned())),
        );
        for refused in ["Paid", "PAID", "paid late", "complete"] {
            assert_eq!(
                Field::InstalmentStatus.kind().normalise(refused),
                None,
                "`{refused}` is not an instalment status",
            );
        }

        assert_eq!(
            Field::CertificateStatus.kind().normalise("issued"),
            Some(Reading::Text("issued".to_owned())),
        );
        assert_eq!(Field::CertificateStatus.kind().normalise("Issued"), None);
    }

    /// One list per vocabulary field, and a field of any other kind has none:
    /// the schema's `enum` and this module's check read the same slice, so a
    /// field with an empty or absent vocabulary could be advertised as
    /// accepting nothing at all.
    #[test]
    fn every_vocabulary_field_declares_the_words_it_reads() {
        let mut vocabulary_fields = 0usize;
        for subject_type in SubjectType::ALL {
            for field in subject_type.fields() {
                match field.kind().vocabulary() {
                    Some(words) => {
                        vocabulary_fields += 1;
                        assert!(
                            !words.is_empty(),
                            "`{}` reads a vocabulary of no words",
                            field.as_str(),
                        );
                        let mut sorted = words.to_vec();
                        sorted.sort_unstable();
                        sorted.dedup();
                        assert_eq!(
                            sorted.len(),
                            words.len(),
                            "`{}` lists a word twice: {words:?}",
                            field.as_str(),
                        );
                        for word in words {
                            assert_eq!(
                                field.kind().normalise(word),
                                Some(Reading::Text((*word).to_owned())),
                                "`{word}` is advertised for `{}` but not read",
                                field.as_str(),
                            );
                        }
                    }
                    None => assert!(
                        !matches!(field.kind(), FieldKind::Vocabulary(_)),
                        "`{}` is a vocabulary field with no vocabulary",
                        field.as_str(),
                    ),
                }
            }
        }
        assert_eq!(
            vocabulary_fields, 3,
            "the vocabulary fields are a milestone's status, a certificate's and an \
             instalment's",
        );
    }

    /// Every rule a caller is sent shows the canonical form, not just names
    /// it: the one thing most likely to stop a caller sending whole dirhams.
    #[test]
    fn every_rule_shows_the_form_it_asks_for() {
        for (kind, example) in [
            (Field::PaidToDateMinor.kind(), "147000000"),
            (Field::MilestoneActualDate.kind(), "2026-09-04"),
            (Field::MilestonePercentComplete.kind(), "48"),
            (Field::HasOverdueInstalment.kind(), "true"),
            (Field::MilestoneStatus.kind(), "in_progress"),
        ] {
            assert!(
                kind.rule().contains(example),
                "the rule for {kind:?} must show `{example}`: {}",
                kind.rule(),
            );
        }
        assert!(
            Field::PaidToDateMinor
                .kind()
                .rule()
                .contains("1,470,000.00 AED is written `147000000`"),
            "the money rule must carry a worked conversion: {}",
            Field::PaidToDateMinor.kind().rule(),
        );
    }
}
