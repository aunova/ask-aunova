//! The project record (`project.json`) as the server reads it.
//!
//! `dataset_manifest::project::Project` deliberately keeps only what the
//! signing tools consume — `doc_id`, `path`, `issuer` — because a field nobody
//! reads is dead weight in a signing tool. The server answers questions, so it
//! needs the rest: milestones, certificates, payment schedules, the escrow
//! block, document titles and `visibility`. Those types live here rather than
//! in the shared crate.
//!
//! Nothing in this module decides whether the record may be trusted. It is
//! parsed only after [`crate::verify`] has verified the `PROJECT-RECORD`
//! signature over the file's exact bytes in the same call.
//!
//! **Money is an integer.** Every amount is minor units (fils) in an `i64` and
//! is never parsed, formatted or compared as a floating-point number. An amount
//! outside `i64` fails to parse, which refuses the whole record, rather than
//! being silently rounded.

use serde::Deserialize;

use crate::auth::Role;

/// The `doc_id` under which the record signs itself.
///
/// The record carries the milestones, the payments and the escrow balance, so
/// every tool refuses when this document does not verify.
pub const PROJECT_RECORD_DOC_ID: &str = "PROJECT-RECORD";

/// A validated-on-parse view of `project.json`.
///
/// Unknown fields are ignored: the dataset also carries a watermark, a
/// disclaimer and a schema version that the signing tools check and this server
/// does not read.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectRecord {
    /// Headline facts about the project itself.
    pub project: ProjectHeader,
    /// The currency every amount in this record is denominated in.
    pub currency: Currency,
    /// The fictional organisations that issue the source documents.
    pub issuers: Vec<RecordIssuer>,
    /// Construction milestones, in the record's own order.
    pub milestones: Vec<Milestone>,
    /// Certificates, issued and otherwise.
    pub certificates: Vec<Certificate>,
    /// The escrow account and its latest attestation.
    pub escrow: Escrow,
    /// One payment schedule per unit.
    pub payment_schedules: Vec<PaymentSchedule>,
    /// Every source document the dataset declares.
    pub documents: Vec<RecordDocument>,
}

impl ProjectRecord {
    /// The display name of an issuer id, or the id itself when the record does
    /// not name it.
    pub fn issuer_name(&self, issuer_id: &str) -> String {
        self.issuers
            .iter()
            .find(|issuer| issuer.id == issuer_id)
            .map_or_else(|| issuer_id.to_owned(), |issuer| issuer.name.clone())
    }

    /// The date the record puts on a document, when it states one.
    ///
    /// Only certificates carry a date of their own; for anything else the
    /// record states none, and an evidence card says so rather than borrowing
    /// an unrelated date.
    pub fn document_date(&self, doc_id: &str) -> Option<&str> {
        self.certificates
            .iter()
            .find(|certificate| certificate.document == doc_id)
            .and_then(|certificate| certificate.issued_date.as_deref())
    }

    /// The payment schedule for one unit, if the record holds one.
    pub fn schedule_for(&self, unit: &str) -> Option<&PaymentSchedule> {
        self.payment_schedules
            .iter()
            .find(|schedule| schedule.unit == unit)
    }
}

/// Headline facts about the project.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectHeader {
    /// Display name, e.g. `Palm Vista Tower`.
    pub name: String,
    /// Lifecycle status, e.g. `under_construction`.
    pub status: String,
    /// The date the record describes.
    pub as_of: String,
    /// Contractual handover date.
    pub expected_handover: String,
}

/// The currency of every amount in the record.
#[derive(Debug, Clone, Deserialize)]
pub struct Currency {
    /// ISO-style code, e.g. `AED`.
    pub code: String,
    /// How many minor units make one major unit, e.g. 100 fils to the dirham.
    pub minor_units_per_major: i64,
}

impl Currency {
    /// How many decimal places this currency's minor unit gives it.
    ///
    /// Derived from the record's own `minor_units_per_major`, so a currency
    /// with no minor unit has none and a three-decimal currency has three.
    pub const fn decimal_places(&self) -> usize {
        let per = self.minor_units_per_major;
        if per <= 1 {
            return 0;
        }
        let mut places = 0usize;
        let mut remaining = per.unsigned_abs();
        while remaining > 1 {
            remaining /= 10;
            places += 1;
        }
        places
    }

    /// Render minor units as major units, by integer arithmetic only.
    ///
    /// No floating-point value is constructed at any point, so the digits
    /// printed are exactly the digits stored.
    pub fn format_minor(&self, amount_minor: i64) -> String {
        let per = self.minor_units_per_major;
        if per <= 1 {
            return format!("{amount_minor} {}", self.code);
        }
        let magnitude = amount_minor.unsigned_abs();
        let per = per.unsigned_abs();
        let major = magnitude / per;
        let minor = magnitude % per;
        let places = self.decimal_places();
        let sign = if amount_minor < 0 { "-" } else { "" };
        format!("{sign}{major}.{minor:0places$} {}", self.code)
    }
}

/// A fictional issuer named by the record.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordIssuer {
    /// Issuer id, as used by `manifest.json` and `keys/public/issuers.json`.
    pub id: String,
    /// Display name.
    pub name: String,
}

/// One construction milestone.
#[derive(Debug, Clone, Deserialize)]
pub struct Milestone {
    /// Milestone id, e.g. `MS-07`.
    pub id: String,
    /// What the milestone covers.
    pub title: String,
    /// `complete`, `in_progress` or `not_started`.
    pub status: String,
    /// Programme date.
    pub planned_date: String,
    /// The date it actually completed, when it has.
    pub actual_date: Option<String>,
    /// Progress within an in-progress milestone, in whole percent.
    pub percent_complete: Option<u32>,
    /// Share of the whole works this milestone represents, in whole percent.
    pub percent_of_works: u32,
    /// `doc_id`s of the documents that evidence this milestone.
    pub evidence: Vec<String>,
    /// `doc_id` of the internal contractor note behind it, when there is one.
    pub contractor_note: Option<String>,
    /// Who may hear about this milestone.
    pub visibility: String,
}

/// One certificate held (or awaited) for the project.
#[derive(Debug, Clone, Deserialize)]
pub struct Certificate {
    /// Certificate id, e.g. `CERT-FIRE-2026-087`.
    pub id: String,
    /// What the certificate attests.
    pub title: String,
    /// Issuer id of the fictional organisation that issued it.
    pub issuer: String,
    /// `issued`, `pending`, or any other state the record records.
    pub status: String,
    /// Date of issue, absent while the certificate is not issued.
    pub issued_date: Option<String>,
    /// Expiry date, absent while the certificate is not issued.
    pub expires_date: Option<String>,
    /// Scope of the attestation, in the issuer's words.
    pub scope: String,
    /// `doc_id` of the certificate document itself.
    pub document: String,
    /// Who may hear about this certificate.
    pub visibility: String,
}

/// The escrow account holding buyer instalments.
#[derive(Debug, Clone, Deserialize)]
pub struct Escrow {
    /// Account reference.
    pub account_ref: String,
    /// Issuer id of the trustee.
    pub trustee: String,
    /// `doc_id` of the most recent attestation certificate.
    pub latest_attestation: String,
    /// Attested balance in minor units.
    pub attested_balance_minor: i64,
    /// The date the balance was attested as at.
    pub attested_as_of: String,
}

/// One unit's payment schedule.
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentSchedule {
    /// Unit number as printed in the sale agreement.
    pub unit: String,
    /// Unit description.
    #[serde(rename = "type")]
    pub unit_type: String,
    /// Contract price in minor units.
    pub total_price_minor: i64,
    /// The escrow account this unit's instalments are paid into.
    pub escrow_account_ref: String,
    /// Paid so far, in minor units.
    pub paid_to_date_minor: i64,
    /// `doc_id` of the signed payment schedule document.
    pub document: String,
    /// The instalments, in the record's own order.
    pub instalments: Vec<Instalment>,
}

/// One instalment of a payment schedule.
#[derive(Debug, Clone, Deserialize)]
pub struct Instalment {
    /// Instalment id within the schedule, e.g. `IN-06`.
    pub id: String,
    /// What triggers it, in words.
    pub label: String,
    /// Milestone id or `contract` that triggers it.
    pub trigger: String,
    /// Share of the contract price, in whole percent.
    pub percent: u32,
    /// Amount in minor units.
    pub amount_minor: i64,
    /// Date it falls due.
    pub due_date: String,
    /// `paid`, `paid_late`, `overdue`, `scheduled`, or whatever the record says.
    pub status: String,
    /// The date it was paid, when it has been.
    pub paid_date: Option<String>,
}

/// One declared source document.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordDocument {
    /// Document id, as signed in `manifest.json`.
    pub doc_id: String,
    /// Human-readable title.
    pub title: String,
    /// Path relative to the dataset directory.
    pub path: String,
    /// Issuer id.
    pub issuer: String,
    /// Who may read it.
    pub visibility: String,
}

/// Who may hear a fact, as written in the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visibility {
    /// Anyone with a token.
    Public,
    /// The project developer only.
    Developer,
    /// The buyer of one unit, and the developer.
    Unit(String),
    /// A value this build does not understand.
    ///
    /// It is shown to nobody. A vocabulary the server cannot read must never
    /// widen what a caller hears, so an unrecognised value fails closed rather
    /// than defaulting to `public`.
    Unrecognised(String),
}

impl Visibility {
    /// Read a `visibility` value from the record.
    pub fn parse(value: &str) -> Self {
        match value {
            "public" => Visibility::Public,
            "developer" => Visibility::Developer,
            other => match other.strip_prefix("unit:") {
                Some(unit) if !unit.is_empty() => Visibility::Unit(unit.to_owned()),
                _ => Visibility::Unrecognised(other.to_owned()),
            },
        }
    }

    /// Whether `role` may hear something carrying this visibility.
    pub fn visible_to(&self, role: &Role) -> bool {
        match (self, role) {
            (Visibility::Public, _) => true,
            (Visibility::Developer, Role::Developer) => true,
            (Visibility::Developer, Role::Buyer { .. }) => false,
            (Visibility::Unit(_), Role::Developer) => true,
            (Visibility::Unit(unit), Role::Buyer { unit: own }) => unit == own,
            (Visibility::Unrecognised(_), _) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aed() -> Currency {
        Currency {
            code: "AED".to_owned(),
            minor_units_per_major: 100,
        }
    }

    #[test]
    fn minor_units_are_formatted_by_integer_arithmetic() {
        let currency = aed();
        assert_eq!(currency.format_minor(24_500_000), "245000.00 AED");
        assert_eq!(currency.format_minor(48_213_900_000), "482139000.00 AED");
        assert_eq!(currency.format_minor(1), "0.01 AED");
        assert_eq!(currency.format_minor(0), "0.00 AED");
        assert_eq!(currency.format_minor(-1), "-0.01 AED");
        assert_eq!(currency.format_minor(i64::MAX), "92233720368547758.07 AED");
        assert_eq!(currency.format_minor(i64::MIN), "-92233720368547758.08 AED");
    }

    #[test]
    fn an_indivisible_currency_prints_whole_units() {
        let currency = Currency {
            code: "XTS".to_owned(),
            minor_units_per_major: 1,
        };
        assert_eq!(currency.format_minor(42), "42 XTS");
    }

    #[test]
    fn a_three_decimal_currency_takes_its_places_from_the_record() {
        let currency = Currency {
            code: "XTS".to_owned(),
            minor_units_per_major: 1000,
        };
        assert_eq!(currency.format_minor(1_234), "1.234 XTS");
        assert_eq!(currency.format_minor(7), "0.007 XTS");
    }

    #[test]
    fn visibility_parses_the_vocabulary_the_dataset_uses() {
        assert_eq!(Visibility::parse("public"), Visibility::Public);
        assert_eq!(Visibility::parse("developer"), Visibility::Developer);
        assert_eq!(
            Visibility::parse("unit:1204"),
            Visibility::Unit("1204".to_owned())
        );
        assert_eq!(
            Visibility::parse("unit:"),
            Visibility::Unrecognised("unit:".to_owned())
        );
        assert_eq!(
            Visibility::parse("everyone"),
            Visibility::Unrecognised("everyone".to_owned())
        );
    }

    #[test]
    fn a_buyer_hears_public_facts_and_their_own_unit_only() {
        let buyer = Role::Buyer {
            unit: "1204".to_owned(),
        };
        assert!(Visibility::Public.visible_to(&buyer));
        assert!(!Visibility::Developer.visible_to(&buyer));
        assert!(Visibility::Unit("1204".to_owned()).visible_to(&buyer));
        assert!(!Visibility::Unit("0802".to_owned()).visible_to(&buyer));
    }

    #[test]
    fn the_developer_hears_every_understood_visibility() {
        let developer = Role::Developer;
        assert!(Visibility::Public.visible_to(&developer));
        assert!(Visibility::Developer.visible_to(&developer));
        assert!(Visibility::Unit("0802".to_owned()).visible_to(&developer));
    }

    #[test]
    fn an_unrecognised_visibility_is_shown_to_nobody() {
        let unknown = Visibility::Unrecognised("auditor".to_owned());
        assert!(!unknown.visible_to(&Role::Developer));
        assert!(!unknown.visible_to(&Role::Buyer {
            unit: "1204".to_owned()
        }));
    }

    #[test]
    fn the_record_names_issuers_and_dates_only_where_it_has_them() {
        let record: ProjectRecord = serde_json::from_str(
            r#"{
                "project": {"name": "T", "status": "under_construction",
                            "as_of": "2026-10-19", "expected_handover": "2027-08-31"},
                "currency": {"code": "AED", "minor_units_per_major": 100},
                "issuers": [{"id": "iss", "name": "Issuer Name"}],
                "milestones": [],
                "certificates": [{"id": "C1", "title": "t", "issuer": "iss",
                                  "status": "issued", "issued_date": "2026-09-04",
                                  "expires_date": null, "scope": "s",
                                  "document": "DOC-1", "visibility": "public"}],
                "escrow": {"account_ref": "E", "trustee": "iss",
                           "latest_attestation": "DOC-1",
                           "attested_balance_minor": 1, "attested_as_of": "2026-09-30"},
                "payment_schedules": [],
                "documents": []
            }"#,
        )
        .expect("the fixture record parses");

        assert_eq!(record.issuer_name("iss"), "Issuer Name");
        assert_eq!(record.issuer_name("nobody"), "nobody");
        assert_eq!(record.document_date("DOC-1"), Some("2026-09-04"));
        assert_eq!(record.document_date("DOC-2"), None);
        assert!(record.schedule_for("1204").is_none());
    }

    #[test]
    fn an_amount_too_large_for_an_integer_refuses_the_record() {
        // Far beyond i64, so serde_json reads it as a floating-point literal:
        // the point is that it is refused rather than silently rounded into a
        // balance this server would then state.
        let error = serde_json::from_str::<ProjectRecord>(
            r#"{
                "project": {"name": "T", "status": "s", "as_of": "d", "expected_handover": "d"},
                "currency": {"code": "AED", "minor_units_per_major": 100},
                "issuers": [], "milestones": [], "certificates": [],
                "escrow": {"account_ref": "E", "trustee": "t", "latest_attestation": "D",
                           "attested_balance_minor": 170141183460469231731687303715884105727,
                           "attested_as_of": "d"},
                "payment_schedules": [], "documents": []
            }"#,
        )
        .expect_err("an out-of-range amount must not parse");
        assert!(
            error.to_string().contains("expected i64"),
            "unexpected parse error: {error}",
        );
    }
}
