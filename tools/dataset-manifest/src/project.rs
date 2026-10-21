//! `project.json` — the dataset's own description of itself.
//!
//! Parsed into `Option` fields first and validated afterwards, rather than
//! leaning on serde's "missing field" error, because serde stops at the first
//! problem and names the field but not the document. The dataset author wants
//! every problem at once, each naming the entry it came from.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::error::{DatasetError, display_path};
use crate::files::{JSON_READ_LIMIT, read_capped};

/// Every file in this repository is synthetic demonstration data. A dataset
/// that does not say so is not one these tools will sign.
pub const SYNTHETIC_WATERMARK: &str = "SYNTHETIC DEMO DATA";

/// Name of the dataset description inside a dataset directory.
pub const PROJECT_FILE: &str = "project.json";

/// Subdirectory of a dataset that holds the source documents.
pub const DOCS_DIR: &str = "docs";

#[derive(Deserialize)]
struct RawProject {
    watermark: Option<String>,
    issuers: Option<Vec<RawIssuer>>,
    documents: Option<Vec<RawDocument>>,
    // The sections that cite documents as evidence. Only the citing fields are
    // modelled: every other field of a milestone, a certificate or a schedule
    // is the reading consumer's business, and a type this crate does not need
    // would make these tools refuse records they have no stake in.
    milestones: Option<Vec<RawMilestone>>,
    certificates: Option<Vec<RawCertificate>>,
    payment_schedules: Option<Vec<RawPaymentSchedule>>,
    escrow: Option<RawEscrow>,
}

#[derive(Deserialize)]
struct RawIssuer {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawDocument {
    doc_id: Option<String>,
    title: Option<String>,
    path: Option<String>,
    issuer: Option<String>,
    visibility: Option<String>,
}

#[derive(Deserialize)]
struct RawMilestone {
    id: Option<String>,
    evidence: Option<Vec<String>>,
    contractor_note: Option<String>,
}

#[derive(Deserialize)]
struct RawCertificate {
    id: Option<String>,
    document: Option<String>,
}

#[derive(Deserialize)]
struct RawPaymentSchedule {
    unit: Option<String>,
    document: Option<String>,
}

#[derive(Deserialize)]
struct RawEscrow {
    latest_attestation: Option<String>,
}

/// A fictional document issuer.
#[derive(Debug, Clone)]
pub struct Issuer {
    pub id: String,
    pub name: String,
}

/// One source document.
///
/// `title` and `visibility` are required of the dataset — a missing one is
/// named as an error — but they are not kept here, because nothing the signing
/// tools do consumes them. The signature binds `doc_id`, `issuer`, `path` and
/// the content hash; a struct field nobody reads would be dead weight.
#[derive(Debug, Clone)]
pub struct Document {
    pub doc_id: String,
    pub path: String,
    pub issuer: String,
}

/// One `doc_id` the record cites as evidence, and where it cites it from.
///
/// A citation is not a declaration: `documents` is what gets signed, and a
/// `doc_id` that appears only in a citation has no path, no issuer and no
/// signature. Collecting the citations is what lets a verifier say so.
#[derive(Debug, Clone)]
pub struct EvidenceReference {
    /// The `doc_id` as cited.
    pub doc_id: String,
    /// The field it was cited from, named the way the dataset author wrote
    /// it: `milestones[6] (id "MS-07") "evidence"`.
    pub cited_by: String,
}

/// A validated `project.json`.
///
/// The watermark is checked during validation and not kept: once validation
/// has passed it is known to equal [`SYNTHETIC_WATERMARK`], so a field holding
/// a copy of a constant would be dead weight.
#[derive(Debug, Clone)]
pub struct Project {
    pub issuers: Vec<Issuer>,
    pub documents: Vec<Document>,
    /// Every `doc_id` the record cites as evidence for something, in the order
    /// the record cites them. Whether each one is declared is a verifier's
    /// question, not a parser's, so nothing is rejected here.
    pub evidence_references: Vec<EvidenceReference>,
}

impl Project {
    /// Read and validate `<dataset_dir>/project.json`.
    pub fn load(dataset_dir: &Path) -> Result<Self, DatasetError> {
        let path = dataset_dir.join(PROJECT_FILE);
        let text = read_capped(&path, "the dataset description", JSON_READ_LIMIT)?;
        Self::from_text(&text, &display_path(&path))
    }

    /// Validate a `project.json` whose bytes the caller already holds.
    ///
    /// Verification hashes a buffer and then has to parse *that* buffer: a
    /// second read of the same path is a second set of bytes. `where_from`
    /// names the file those bytes came from, for the error messages.
    pub fn from_text(text: &str, where_from: &str) -> Result<Self, DatasetError> {
        let raw: RawProject = serde_json::from_str(text).map_err(|source| DatasetError::Json {
            what: "the dataset description",
            path: where_from.to_string(),
            source,
        })?;
        Self::validate(raw, where_from)
    }

    /// Location of a document on disk, given the dataset directory.
    pub fn document_path(dataset_dir: &Path, document_path: &str) -> PathBuf {
        dataset_dir.join(document_path)
    }

    fn validate(raw: RawProject, where_from: &str) -> Result<Self, DatasetError> {
        let evidence_references = evidence_references(&raw);
        let mut problems = Vec::new();

        match raw.watermark {
            Some(watermark) if watermark == SYNTHETIC_WATERMARK => {}
            Some(other) => problems.push(format!(
                "\"watermark\" is {other:?}, but these tools only sign datasets watermarked {SYNTHETIC_WATERMARK:?}"
            )),
            None => problems.push(format!(
                "\"watermark\" is missing; it must be {SYNTHETIC_WATERMARK:?}"
            )),
        }

        let mut issuers = Vec::new();
        let mut issuer_ids = BTreeSet::new();
        match raw.issuers {
            None => problems.push("\"issuers\" is missing".to_string()),
            Some(raw_issuers) => {
                if raw_issuers.is_empty() {
                    problems.push("\"issuers\" is empty".to_string());
                }
                for (index, raw_issuer) in raw_issuers.into_iter().enumerate() {
                    let label = format!("issuers[{index}]");
                    let Some(id) = require(&mut problems, &label, "id", raw_issuer.id) else {
                        continue;
                    };
                    let label = format!("issuers[{index}] (id {id:?})");
                    let Some(name) = require(&mut problems, &label, "name", raw_issuer.name) else {
                        continue;
                    };
                    if !issuer_ids.insert(id.clone()) {
                        problems.push(format!("{label} repeats an issuer id already declared"));
                        continue;
                    }
                    issuers.push(Issuer { id, name });
                }
            }
        }

        let mut documents = Vec::new();
        let mut doc_ids = BTreeSet::new();
        match raw.documents {
            None => problems.push("\"documents\" is missing".to_string()),
            Some(raw_documents) => {
                if raw_documents.is_empty() {
                    problems.push("\"documents\" is empty".to_string());
                }
                for (index, raw_document) in raw_documents.into_iter().enumerate() {
                    let label = format!("documents[{index}]");
                    let doc_id = require(&mut problems, &label, "doc_id", raw_document.doc_id);
                    let label = match &doc_id {
                        Some(doc_id) => format!("documents[{index}] (doc_id {doc_id:?})"),
                        None => label,
                    };
                    // All five contract fields are required. `title` and
                    // `visibility` are checked and then dropped.
                    let title = require(&mut problems, &label, "title", raw_document.title);
                    let path = require(&mut problems, &label, "path", raw_document.path);
                    let issuer = require(&mut problems, &label, "issuer", raw_document.issuer);
                    let visibility =
                        require(&mut problems, &label, "visibility", raw_document.visibility);

                    let (Some(doc_id), Some(_title), Some(path), Some(issuer), Some(_visibility)) =
                        (doc_id, title, path, issuer, visibility)
                    else {
                        continue;
                    };

                    if let Some(problem) = unsafe_document_path(&path) {
                        problems.push(format!("{label} has a \"path\" that {problem}"));
                        continue;
                    }
                    if !issuer_ids.contains(&issuer) {
                        problems.push(format!(
                            "{label} names issuer {issuer:?}, which is not declared in \"issuers\""
                        ));
                        continue;
                    }
                    if !doc_ids.insert(doc_id.clone()) {
                        problems.push(format!("{label} repeats a doc_id already declared"));
                        continue;
                    }
                    documents.push(Document {
                        doc_id,
                        path,
                        issuer,
                    });
                }
            }
        }

        if problems.is_empty() {
            Ok(Project {
                issuers,
                documents,
                evidence_references,
            })
        } else {
            Err(DatasetError::Invalid {
                what: "the dataset description",
                path: where_from.to_string(),
                problems,
            })
        }
    }
}

/// Every `doc_id` the record cites, from every field that cites one.
///
/// The five citing fields are the five the reading consumer resolves against
/// `documents`: a milestone's `evidence` list and its `contractor_note`, a
/// certificate's `document`, a payment schedule's `document`, and the escrow's
/// `latest_attestation`. A field added to the record and not added here is a
/// citation this verifier would pass as sound, so this list and the reader's
/// are checked against each other in `tests/sign_verify.rs`.
fn evidence_references(raw: &RawProject) -> Vec<EvidenceReference> {
    let mut references = Vec::new();
    let mut cite = |doc_id: &str, cited_by: String| {
        references.push(EvidenceReference {
            doc_id: doc_id.to_owned(),
            cited_by,
        });
    };

    for (index, milestone) in raw.milestones.iter().flatten().enumerate() {
        let label = labelled("milestones", index, "id", milestone.id.as_deref());
        for doc_id in milestone.evidence.iter().flatten() {
            cite(doc_id, format!("{label} \"evidence\""));
        }
        if let Some(doc_id) = &milestone.contractor_note {
            cite(doc_id, format!("{label} \"contractor_note\""));
        }
    }
    for (index, certificate) in raw.certificates.iter().flatten().enumerate() {
        if let Some(doc_id) = &certificate.document {
            let label = labelled("certificates", index, "id", certificate.id.as_deref());
            cite(doc_id, format!("{label} \"document\""));
        }
    }
    for (index, schedule) in raw.payment_schedules.iter().flatten().enumerate() {
        if let Some(doc_id) = &schedule.document {
            let label = labelled("payment_schedules", index, "unit", schedule.unit.as_deref());
            cite(doc_id, format!("{label} \"document\""));
        }
    }
    if let Some(doc_id) = raw
        .escrow
        .as_ref()
        .and_then(|escrow| escrow.latest_attestation.as_ref())
    {
        cite(doc_id, "\"escrow\" \"latest_attestation\"".to_owned());
    }

    references
}

/// `milestones[6] (id "MS-07")`, or `milestones[6]` when the entry has no id
/// to name it by.
fn labelled(section: &str, index: usize, field: &str, id: Option<&str>) -> String {
    match id {
        Some(id) => format!("{section}[{index}] ({field} {id:?})"),
        None => format!("{section}[{index}]"),
    }
}

fn require(
    problems: &mut Vec<String>,
    label: &str,
    field: &str,
    value: Option<String>,
) -> Option<String> {
    match value {
        Some(value) if !value.is_empty() => Some(value),
        Some(_) => {
            problems.push(format!("{label} has an empty \"{field}\""));
            None
        }
        None => {
            problems.push(format!("{label} is missing the required field \"{field}\""));
            None
        }
    }
}

/// Reject any document path that is not a plain relative path inside the
/// dataset directory. These paths are opened and hashed, so `../` or a leading
/// `/` would reach outside the dataset.
fn unsafe_document_path(path: &str) -> Option<&'static str> {
    let mut components = Path::new(path).components().peekable();
    if components.peek().is_none() {
        return Some("is empty");
    }
    for component in components {
        match component {
            Component::Normal(_) => {}
            Component::ParentDir => return Some("contains \"..\""),
            Component::CurDir => return Some("contains \".\""),
            Component::RootDir | Component::Prefix(_) => return Some("is absolute"),
        }
    }
    None
}

/// The dataset's name, taken from the last component of its directory path.
///
/// The name is part of every signing payload, so an entry signed for one
/// dataset cannot be replayed into another.
pub fn dataset_name(dataset_dir: &Path) -> Result<String, DatasetError> {
    dataset_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| DatasetError::NamelessDataset {
            path: display_path(dataset_dir),
        })
}
