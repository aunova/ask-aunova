//! Live verification of the signed dataset.
//!
//! Every tool call and every resource read runs [`DatasetConfig::load_and_verify`]
//! from scratch: it recomputes each document's hash, checks each Ed25519
//! signature against the issuer key committed in `keys/public/issuers.json`,
//! and derives each document's status from *that* run.
//!
//! There is no cache, no `verified: true` flag kept anywhere, and no
//! startup-time check whose result is reused. That is deliberate: the only
//! cache key that would be sound here is the content hash of each file, which
//! is the very thing being checked, so caching could only ever hide a change.
//! The dataset is a handful of small files; a full re-verify is cheap, and it
//! means a byte flipped while the server is running changes the next answer.
//!
//! The record is parsed *after* its own signature has been checked, in the same
//! call, from the same file.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use dataset_manifest::{
    project::PROJECT_FILE,
    verify::{DatasetFault, DocumentFault, FailureScope, verify_dataset},
};

use crate::record::{PROJECT_RECORD_DOC_ID, ProjectRecord, Visibility};

/// Environment variable naming the dataset directory.
pub const DATASET_DIR_ENV: &str = "ASK_AUNOVA_DATASET_DIR";
/// Environment variable naming the directory holding `public/issuers.json`.
pub const KEYS_DIR_ENV: &str = "ASK_AUNOVA_KEYS_DIR";
/// Dataset directory used when [`DATASET_DIR_ENV`] is unset, relative to the
/// directory the server is started from (the repository root).
pub const DEFAULT_DATASET_DIR: &str = "data/palm-vista";
/// Keys directory used when [`KEYS_DIR_ENV`] is unset, relative to the
/// directory the server is started from (the repository root).
pub const DEFAULT_KEYS_DIR: &str = "keys";

/// Where the signed dataset and the committed issuer keys live.
///
/// Both default to the repository-relative paths the `verify-dataset` CLI
/// defaults to, and both are overridable so a test can point the server at a
/// dataset of its own instead of mutating the committed one.
#[derive(Clone, Debug)]
pub struct DatasetConfig {
    dataset_dir: PathBuf,
    keys_dir: PathBuf,
}

impl DatasetConfig {
    /// Build a configuration from explicit directories.
    pub fn new(dataset_dir: impl Into<PathBuf>, keys_dir: impl Into<PathBuf>) -> Self {
        Self {
            dataset_dir: dataset_dir.into(),
            keys_dir: keys_dir.into(),
        }
    }

    /// Build a configuration from a name-to-value lookup, falling back to the
    /// repository-relative defaults. The tests pass a fake lookup, which keeps
    /// them free of process-wide mutation.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        Self::new(
            lookup(DATASET_DIR_ENV).unwrap_or_else(|| DEFAULT_DATASET_DIR.to_owned()),
            lookup(KEYS_DIR_ENV).unwrap_or_else(|| DEFAULT_KEYS_DIR.to_owned()),
        )
    }

    /// Build a configuration from the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The dataset directory, as configured.
    pub fn dataset_dir(&self) -> &Path {
        &self.dataset_dir
    }

    /// Verify the dataset and read the record behind it.
    ///
    /// This is blocking file I/O and belongs on a blocking thread; callers in
    /// the request path go through [`crate::handler`], which dispatches it.
    ///
    /// Two things have to hold for an answer to mean anything, and both are
    /// enforced here rather than left to a caller.
    ///
    /// **A dataset-level fault refuses every call.** A repeated `doc_id`, a
    /// bad manifest header, a file signed but not declared: these belong to
    /// the dataset as a whole and leave no single document to mark, so there
    /// is no honest way to answer from such a dataset while reporting the
    /// fault, and nothing is answered at all. A fault belonging to *one
    /// document* is different, and deliberately not fatal: that document is
    /// marked `signature_invalid` or `missing`, no fact rests on it, and
    /// every other document still answers. `verify-dataset` exits non-zero
    /// for either kind, because it asks "is this dataset entirely sound";
    /// this server degrades per document on purpose, so one broken file does
    /// not take away the thirteen that verified.
    ///
    /// **The record read is the record checked** — the same bytes, from one
    /// read, not the same path read twice.
    pub fn load_and_verify(&self) -> Result<VerifiedDataset, DatasetUnavailable> {
        let outcome = verify_dataset(&self.dataset_dir, &self.keys_dir)
            .map_err(|error| self.unreadable(&error))?;

        let faults: Vec<DatasetFaultReport> = outcome
            .dataset_faults()
            .into_iter()
            .map(|(kind, detail)| DatasetFaultReport {
                kind,
                detail: self.redact(&detail),
            })
            .collect();
        if !faults.is_empty() {
            return Err(DatasetUnavailable::DatasetNotVerifiable { faults });
        }

        let declared: BTreeSet<&str> = outcome
            .project
            .documents
            .iter()
            .map(|document| document.doc_id.as_str())
            .collect();

        let mut statuses: BTreeMap<&str, Verdict> = BTreeMap::new();
        for failure in &outcome.failures {
            let FailureScope::Document { doc_id, fault } = failure.scope() else {
                // Unreachable: a dataset-level fault refused above. The match
                // stays exhaustive so a new scope cannot slip through as a
                // clean document.
                continue;
            };
            let status = match fault {
                DocumentFault::Integrity => DocumentStatus::SignatureInvalid,
                DocumentFault::Absent => DocumentStatus::Missing,
            };
            let verdict = statuses.entry(doc_id).or_insert(Verdict {
                status,
                detail: String::new(),
            });
            verdict.status = verdict.status.worse_of(status);
            if !verdict.detail.is_empty() {
                verdict.detail.push_str("; ");
            }
            verdict.detail.push_str(&failure.to_string());
        }

        // A document is verified only when it was *checked* and nothing failed.
        // "No failure was reported" on its own would call an undeclared, and
        // therefore unchecked, document clean.
        let verdict_for = |doc_id: &str| -> Verdict {
            if let Some(verdict) = statuses.get(doc_id) {
                return verdict.clone();
            }
            if declared.contains(doc_id) {
                return Verdict {
                    status: DocumentStatus::Verified,
                    detail: String::new(),
                };
            }
            Verdict {
                status: DocumentStatus::Missing,
                detail: format!(
                    "{doc_id} is not declared by {PROJECT_FILE}, so no signature was checked for it"
                ),
            }
        };

        let record_verdict = verdict_for(PROJECT_RECORD_DOC_ID);
        if record_verdict.status != DocumentStatus::Verified {
            return Err(DatasetUnavailable::ProjectRecordUnverified {
                status: record_verdict.status,
                detail: record_verdict.detail,
            });
        }

        // Only now, with the signature over these exact bytes checked in this
        // same call, is the record worth reading — and it is read from the
        // buffer that was hashed, never from the path again.
        let record: ProjectRecord =
            serde_json::from_str(&outcome.project_json).map_err(|error| {
                DatasetUnavailable::Unreadable {
                    detail: format!("the project record did not parse: {error}"),
                }
            })?;

        let documents = record
            .documents
            .iter()
            .map(|document| {
                let verdict = verdict_for(&document.doc_id);
                let evidence = DocumentEvidence {
                    doc_id: document.doc_id.clone(),
                    title: document.title.clone(),
                    path: document.path.clone(),
                    issuer_id: document.issuer.clone(),
                    issuer_name: record.issuer_name(&document.issuer),
                    visibility: Visibility::parse(&document.visibility),
                    sha256: outcome
                        .manifest
                        .document(&document.doc_id)
                        .map(|entry| entry.sha256.clone()),
                    status: verdict.status,
                    detail: verdict.detail,
                };
                (document.doc_id.clone(), evidence)
            })
            .collect();

        Ok(VerifiedDataset {
            declared: outcome.declared(),
            dataset: outcome.dataset,
            verified: outcome.verified,
            documents,
            record,
        })
    }

    /// Turn a dataset-tooling failure into a refusal a caller may read.
    fn unreadable(&self, error: &dyn std::fmt::Display) -> DatasetUnavailable {
        DatasetUnavailable::Unreadable {
            detail: self.redact(&error.to_string()),
        }
    }

    /// Drop the server's filesystem layout out of a message a caller sees.
    ///
    /// The tooling's messages name the file that failed, which is what an
    /// operator needs — but those paths are built from the configured
    /// directories, and an MCP caller has no business learning the server's
    /// filesystem layout from a refusal. Both configured directories are
    /// replaced by a placeholder, and so is the dataset directory's own name,
    /// which a manifest-header fault would otherwise quote. What is kept is
    /// the part that explains the failure ("trailing characters at line 326").
    fn redact(&self, message: &str) -> String {
        let mut redacted = message
            .replace(&self.dataset_dir.display().to_string(), "<dataset>")
            .replace(&self.keys_dir.display().to_string(), "<keys>");
        if let Some(name) = self.dataset_dir.file_name().and_then(|name| name.to_str()) {
            redacted = redacted.replace(&format!("{name:?}"), "<dataset>");
        }
        redacted
    }
}

/// What one verification run found, with the record it licenses reading.
#[derive(Debug, Clone)]
pub struct VerifiedDataset {
    /// Dataset name, from the dataset directory.
    pub dataset: String,
    /// How many documents the record declares.
    pub declared: usize,
    /// How many of them passed every check in this run.
    pub verified: usize,
    /// Per-document outcome of this run, keyed by `doc_id`.
    pub documents: BTreeMap<String, DocumentEvidence>,
    /// The record, parsed after its own signature verified.
    pub record: ProjectRecord,
}

impl VerifiedDataset {
    /// The evidence for one document, if the record declares it.
    pub fn document(&self, doc_id: &str) -> Option<&DocumentEvidence> {
        self.documents.get(doc_id)
    }

    /// How many documents `role` may read, and how many of those verified in
    /// this run.
    ///
    /// Every answer reports these rather than the dataset-wide totals. A buyer
    /// told "14 declared, 13 verified" has been told that four documents exist
    /// which they may not read, and that one of them is broken — which is
    /// exactly what the per-document role filter exists to prevent. A buyer's
    /// totals describe a buyer's documents and nothing else.
    pub fn counts_for(&self, role: &crate::auth::Role) -> (usize, usize) {
        let visible = self
            .documents
            .values()
            .filter(|evidence| evidence.visibility.visible_to(role));
        let declared = visible.clone().count();
        let verified = visible
            .filter(|evidence| evidence.status == DocumentStatus::Verified)
            .count();
        (declared, verified)
    }
}

/// One document as this verification run found it.
#[derive(Debug, Clone)]
pub struct DocumentEvidence {
    /// Document id.
    pub doc_id: String,
    /// Human-readable title from the record.
    pub title: String,
    /// Path relative to the dataset directory.
    pub path: String,
    /// Issuer id.
    pub issuer_id: String,
    /// Issuer display name.
    pub issuer_name: String,
    /// Who may read it.
    pub visibility: Visibility,
    /// The content hash the manifest records, when the manifest covers it.
    pub sha256: Option<String>,
    /// The blunt summary word for this run.
    pub status: DocumentStatus,
    /// Which check actually failed, verbatim from the verifier. Empty when the
    /// document verified.
    pub detail: String,
}

impl DocumentEvidence {
    /// Whether this document may be cited to `role` as sound evidence: the
    /// caller may see it *and* it verified in this run.
    pub fn citable_to(&self, role: &crate::auth::Role) -> bool {
        self.status == DocumentStatus::Verified && self.visibility.visible_to(role)
    }
}

/// The three words a caller ever sees for a document's integrity.
///
/// Every integrity failure — a hash mismatch, a bad signature, a key that is
/// not the one on record for the issuer — is one word, `signature_invalid`,
/// because a caller must never have to grade severities to know whether the
/// bytes moved. [`DocumentEvidence::detail`] carries which check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentStatus {
    /// Hash and signature both checked out in this run.
    Verified,
    /// The document is there, but it is not the document that was signed.
    SignatureInvalid,
    /// No signed document to check.
    Missing,
}

impl DocumentStatus {
    /// The exact word sent to callers.
    pub const fn as_str(self) -> &'static str {
        match self {
            DocumentStatus::Verified => "verified",
            DocumentStatus::SignatureInvalid => "signature_invalid",
            DocumentStatus::Missing => "missing",
        }
    }

    /// The worse of two outcomes for the same document: an absent document
    /// outranks a broken one, and either outranks a clean check.
    const fn worse_of(self, other: Self) -> Self {
        match (self, other) {
            (DocumentStatus::Missing, _) | (_, DocumentStatus::Missing) => DocumentStatus::Missing,
            (DocumentStatus::SignatureInvalid, _) | (_, DocumentStatus::SignatureInvalid) => {
                DocumentStatus::SignatureInvalid
            }
            (DocumentStatus::Verified, DocumentStatus::Verified) => DocumentStatus::Verified,
        }
    }
}

/// A document's status and the reason behind it.
#[derive(Debug, Clone)]
struct Verdict {
    status: DocumentStatus,
    detail: String,
}

/// Why this server cannot answer from the record at all.
///
/// Every variant's `Display` is the **operator's** view: it quotes what the
/// dataset tooling, serde or the runtime reported, and each of those quotes
/// the thing that broke — a path, an issuer, a key, a panic message, or the
/// offending value out of the record. None of that is a caller's to read.
/// What a caller is told goes through [`DatasetUnavailable::message_for`],
/// which gives this text to the developer alone.
#[derive(Debug, thiserror::Error)]
pub enum DatasetUnavailable {
    /// Verification could not even start, or the record would not parse.
    ///
    /// `detail` is the dataset tooling's message or serde's, and serde's
    /// quotes the value it tripped on with its line and column: a record
    /// whose unit 0802 entry has a string where an `i64` belongs puts 0802's
    /// paid-to-date figure in here. A buyer of another unit may not read
    /// that, so this variant is role-filtered like every other.
    #[error("the signed dataset could not be read: {detail}")]
    Unreadable {
        /// What went wrong, from the dataset tooling. Operator-only.
        detail: String,
    },
    /// The record signs itself, and that signature did not check out.
    ///
    /// `detail` is the verifier's own text for whichever check failed on
    /// `PROJECT-RECORD`, built from the manifest entry carrying that
    /// `doc_id` — including the path and issuer that entry declares, which
    /// in a dataset broken enough to reach here are whatever was signed and
    /// not necessarily `project.json`. Operator-only, like the rest.
    #[error(
        "the project record did not verify ({}): {detail}", status.as_str()
    )]
    ProjectRecordUnverified {
        /// `signature_invalid` or `missing`.
        status: DocumentStatus,
        /// Which check failed. Operator-only.
        detail: String,
    },
    /// The dataset carries a fault that belongs to no single document, so it
    /// is a dataset `verify-dataset` exits non-zero on.
    #[error(
        "this dataset is not one its own verifier accepts ({}): {}",
        faults.len(),
        faults.iter().map(|fault| fault.detail.as_str()).collect::<Vec<&str>>().join("; ")
    )]
    DatasetNotVerifiable {
        /// Each fault: its kind, and its text verbatim from the verifier with
        /// the server's own directories redacted out.
        faults: Vec<DatasetFaultReport>,
    },
    /// The blocking verification task did not finish.
    ///
    /// `detail` is `tokio`'s `JoinError`, which for a panicked task reads
    /// `task <id> panicked with message "<the payload>"`
    /// (`.qa/vendor/tokio-1.53.3/src/runtime/task/error.rs:135-153`). A panic
    /// payload is arbitrary text from wherever it was raised, so it is no
    /// more a caller's to read than serde's is.
    #[error("the verification run did not finish: {detail}")]
    DidNotFinish {
        /// What the runtime reported. Operator-only.
        detail: String,
    },
}

/// One dataset-level fault, as this server holds it.
#[derive(Debug, Clone)]
pub struct DatasetFaultReport {
    /// What kind of fault it is — words that name no file.
    pub kind: DatasetFault,
    /// The verifier's own text, which does name the file that broke it.
    pub detail: String,
}

impl DatasetUnavailable {
    /// The short code a caller sees in a refusal.
    pub const fn code(&self) -> &'static str {
        match self {
            DatasetUnavailable::Unreadable { .. } => "dataset_unreadable",
            DatasetUnavailable::ProjectRecordUnverified { .. } => "project_record_unverified",
            DatasetUnavailable::DatasetNotVerifiable { .. } => "dataset_not_verifiable",
            DatasetUnavailable::DidNotFinish { .. } => "verification_did_not_finish",
        }
    }

    /// What this refusal says to `role`.
    ///
    /// The developer is the operator here, and gets `Display`: the whole
    /// detail, which is what someone who can fix the dataset needs. Nobody
    /// else gets any of it, whatever the variant.
    ///
    /// Every variant carries text from outside this server — the verifier's,
    /// serde's, the runtime's — and every one of those quotes the thing that
    /// broke. A dataset-level fault names the file: telling a buyer
    /// "unsigned file: (docs/contractor-note-CN-14.md)" tells them an
    /// internal document exists. A parse error names the *value*: a record
    /// whose unit 0802 entry has a string where an `i64` belongs tells
    /// buyer:1204 what 0802 has paid. Which of those two a given refusal is
    /// depends on a dataset the issuer signed, not on anything this server
    /// controls, so the filter is not applied per variant on the strength of
    /// what its text is believed to contain. It is applied to all of them.
    ///
    /// That is why there is no catch-all arm below. The arm this replaced
    /// (`other => other.to_string()`) made "say everything" the default for
    /// any variant nobody had thought about, and `Unreadable` was exactly
    /// such a variant. Adding one now does not compile until someone writes
    /// what a non-developer may hear about it.
    pub fn message_for(&self, role: &crate::auth::Role) -> String {
        match role {
            crate::auth::Role::Developer => self.to_string(),
            crate::auth::Role::Buyer { .. } => self.public_message(),
        }
    }

    /// The whole of what a non-developer is told: the kind of failure, in
    /// this enum's own vocabulary, and where the detail went.
    ///
    /// Nothing here is interpolated from a message this server did not
    /// write. The one value that is — [`DocumentStatus::as_str`] — is one of
    /// three fixed words naming no file, no document and no figure.
    fn public_message(&self) -> String {
        // None of these ends in a full stop, for the same reason no variant's
        // `Display` does: both callers put this text into a sentence of their
        // own and punctuate it themselves.
        match self {
            DatasetUnavailable::Unreadable { .. } => "the signed dataset could not be read. \
                 What could not be read in it is recorded where this server is run; it is not \
                 this caller's to read"
                .to_owned(),
            DatasetUnavailable::ProjectRecordUnverified { status, .. } => format!(
                "the project record did not verify ({}). Which check failed is recorded where \
                 this server is run; it is not this caller's to read",
                status.as_str(),
            ),
            DatasetUnavailable::DatasetNotVerifiable { faults } => format!(
                "this dataset is not one its own verifier accepts ({}): {}. What is wrong \
                 with it is recorded where this server is run; it is not this caller's to \
                 read",
                plural(faults.len(), "fault"),
                fault_kinds(faults),
            ),
            DatasetUnavailable::DidNotFinish { .. } => "the verification run did not finish. \
                 What the runtime reported is recorded where this server is run; it is not \
                 this caller's to read"
                .to_owned(),
        }
    }
}

/// `1 fault` / `3 faults`.
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The kinds of a set of faults, counted, naming nothing in the dataset.
fn fault_kinds(faults: &[DatasetFaultReport]) -> String {
    let mut counted: BTreeMap<&'static str, usize> = BTreeMap::new();
    for fault in faults {
        *counted.entry(fault.kind.label()).or_insert(0) += 1;
    }
    counted
        .into_iter()
        .map(|(label, count)| format!("{count} × {label}"))
        .collect::<Vec<String>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use dataset_manifest::verify::CheckFailure;

    use super::*;

    #[test]
    fn the_defaults_are_the_repository_relative_dataset_and_keys() {
        let config = DatasetConfig::from_lookup(|_| None);
        assert_eq!(config.dataset_dir(), Path::new("data/palm-vista"));
        assert_eq!(config.keys_dir, Path::new("keys"));
    }

    #[test]
    fn the_environment_overrides_both_directories() {
        let config = DatasetConfig::from_lookup(|name| match name {
            DATASET_DIR_ENV => Some("fixture/dataset".to_owned()),
            KEYS_DIR_ENV => Some("fixture/keys".to_owned()),
            _ => None,
        });
        assert_eq!(config.dataset_dir(), Path::new("fixture/dataset"));
        assert_eq!(config.keys_dir, Path::new("fixture/keys"));
    }

    /// The server's own vocabulary, over the scope the dataset tooling
    /// decides. The mapping is the only part that lives here: which failures
    /// belong to a document and which to the dataset is one rule, in
    /// `dataset_manifest`, shared with `verify-dataset`.
    #[test]
    fn an_integrity_failure_is_one_word_and_an_absent_file_is_another() {
        let hash = CheckFailure::Hash {
            doc_id: "SR-05".to_owned(),
            path: "docs/sr-05.md".to_owned(),
            recorded: "aa".to_owned(),
            actual: "bb".to_owned(),
        };
        assert_eq!(
            hash.scope(),
            FailureScope::Document {
                doc_id: "SR-05",
                fault: DocumentFault::Integrity
            },
        );
        assert!(
            hash.to_string().contains("but the file on disk hashes to"),
            "the detail must name the check that failed: {hash}",
        );

        let missing = CheckFailure::MissingFile {
            doc_id: "SR-05".to_owned(),
            path: "docs/sr-05.md".to_owned(),
        };
        assert_eq!(
            missing.scope(),
            FailureScope::Document {
                doc_id: "SR-05",
                fault: DocumentFault::Absent
            },
        );
    }

    /// A fault that belongs to the dataset stops the call, rather than being
    /// reported alongside an answer composed from the same dataset.
    #[test]
    fn a_dataset_level_fault_is_a_refusal_with_its_own_code() {
        let refusal = DatasetUnavailable::DatasetNotVerifiable {
            faults: vec![DatasetFaultReport {
                kind: DatasetFault::ManifestSet,
                detail: "manifest header: the manifest lists doc_id \"SR-05\" 2 times".to_owned(),
            }],
        };
        assert_eq!(refusal.code(), "dataset_not_verifiable");
        assert!(
            refusal.to_string().contains("SR-05"),
            "the refusal must name the entry that broke it: {refusal}",
        );
    }

    #[test]
    fn the_worse_of_two_outcomes_never_reports_the_better_one() {
        use DocumentStatus::{Missing, SignatureInvalid, Verified};
        assert_eq!(Verified.worse_of(SignatureInvalid), SignatureInvalid);
        assert_eq!(SignatureInvalid.worse_of(Verified), SignatureInvalid);
        assert_eq!(SignatureInvalid.worse_of(Missing), Missing);
        assert_eq!(Missing.worse_of(SignatureInvalid), Missing);
        assert_eq!(Verified.worse_of(Verified), Verified);
    }

    #[test]
    fn the_refusal_codes_are_stable() {
        assert_eq!(
            DatasetUnavailable::Unreadable {
                detail: "x".to_owned()
            }
            .code(),
            "dataset_unreadable",
        );
        let unverified = DatasetUnavailable::ProjectRecordUnverified {
            status: DocumentStatus::SignatureInvalid,
            detail: "hash: PROJECT-RECORD".to_owned(),
        };
        assert_eq!(unverified.code(), "project_record_unverified");
        assert!(
            unverified.to_string().contains("signature_invalid"),
            "the refusal must name the status: {unverified}",
        );
    }

    /// No variant hands a non-developer the text it was built from.
    ///
    /// Every detail here carries the same marker, and the marker stands for
    /// whatever the dataset tooling, serde or the runtime put in that field:
    /// unit 0802's paid-to-date figure, the path of a document a buyer may
    /// not read, a panic payload. A buyer sees none of it and is still told
    /// which kind of failure it was; the developer sees all of it, which is
    /// what makes this a filter rather than a deletion.
    #[test]
    fn no_refusal_hands_a_buyer_the_text_it_was_built_from() {
        const MARKER: &str = "89100000";
        let buyer = crate::auth::Role::Buyer {
            unit: "1204".to_owned(),
        };
        let developer = crate::auth::Role::Developer;

        let refusals = [
            DatasetUnavailable::Unreadable {
                detail: format!(
                    "the project record did not parse: invalid type: string \"{MARKER}\", \
                     expected i64 at line 279 column 38"
                ),
            },
            DatasetUnavailable::ProjectRecordUnverified {
                status: DocumentStatus::SignatureInvalid,
                detail: format!("hash: PROJECT-RECORD (docs/{MARKER}.md)"),
            },
            DatasetUnavailable::DatasetNotVerifiable {
                faults: vec![DatasetFaultReport {
                    kind: DatasetFault::ManifestSet,
                    detail: format!("manifest header: {MARKER}"),
                }],
            },
            DatasetUnavailable::DidNotFinish {
                detail: format!("task 12 panicked with message \"{MARKER}\""),
            },
        ];

        for refusal in refusals {
            let told = refusal.message_for(&buyer);
            assert!(
                !told.contains(MARKER),
                "`{}` handed a buyer the detail it was built from: {told}",
                refusal.code(),
            );
            assert!(
                !told.is_empty(),
                "`{}` must still tell a buyer which kind of failure it was",
                refusal.code(),
            );
            let operator = refusal.message_for(&developer);
            assert!(
                operator.contains(MARKER),
                "`{}` must still give the operator the detail: {operator}",
                refusal.code(),
            );
        }
    }
}
