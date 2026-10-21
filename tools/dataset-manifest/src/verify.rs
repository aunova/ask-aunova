//! Verifying a dataset: recompute every hash, check every signature, and —
//! the line that makes the rest worth anything — check every public key in the
//! manifest against the committed list in `keys/public/issuers.json`.
//!
//! Without that cross-check an attacker who can edit `manifest.json` simply
//! re-signs each entry with a key of their own and the verifier reports a clean
//! dataset.
//!
//! Every failure is collected; the caller reports all of them.

use std::collections::BTreeSet;
use std::path::Path;

use thiserror::Error;

use crate::canonical::signing_payload;
use crate::error::{DatasetError, display_path};
use crate::files::{JSON_READ_LIMIT, list_files_relative, read_capped, sha256_bytes, sha256_file};
use crate::issuers::{ISSUERS_FILE, IssuerKeys};
use crate::keys::verify_payload;
use crate::manifest::Manifest;
use crate::project::{DOCS_DIR, PROJECT_FILE, Project, dataset_name};

/// One failed check. Every variant that concerns a document names both its
/// `doc_id` and its path, because an operator needs to find the file.
#[derive(Debug, Error)]
pub enum CheckFailure {
    #[error(
        "hash: {doc_id} ({path}) — the manifest records sha256 {recorded}, \
         but the file on disk hashes to {actual}"
    )]
    Hash {
        doc_id: String,
        path: String,
        recorded: String,
        actual: String,
    },

    #[error(
        "signature: {doc_id} ({path}) — the Ed25519 signature in the manifest \
         does not verify over the canonical payload under the public key the \
         manifest records for issuer \"{issuer}\""
    )]
    Signature {
        doc_id: String,
        path: String,
        issuer: String,
    },

    #[error(
        "unknown key: {doc_id} ({path}) — the manifest signs for issuer \
         \"{issuer}\" with public key {manifest_key}, which is not the key \
         recorded for that issuer in {ISSUERS_FILE} ({recorded})"
    )]
    UnknownKey {
        doc_id: String,
        path: String,
        issuer: String,
        manifest_key: String,
        recorded: String,
    },

    #[error(
        "missing file: {doc_id} ({path}) — the manifest signs this document but it is not in the dataset"
    )]
    MissingFile { doc_id: String, path: String },

    #[error(
        "missing from manifest: {doc_id} ({path}) — project.json declares this \
         document but the manifest does not sign it"
    )]
    MissingFromManifest { doc_id: String, path: String },

    #[error(
        "unsigned file: ({path}) — present under {DOCS_DIR}/ but not covered by \
         the manifest"
    )]
    UnsignedFile { path: String },

    #[error(
        "not in project.json: {doc_id} ({path}) — the manifest signs a document \
         the dataset does not declare"
    )]
    NotInProject { doc_id: String, path: String },

    #[error("malformed entry: {doc_id} ({path}) — {problem}")]
    Malformed {
        doc_id: String,
        path: String,
        problem: String,
    },

    #[error("manifest header: {problem}")]
    Header { problem: String },

    #[error(
        "dangling evidence: {doc_id} (cited by {cited_by}) — {PROJECT_FILE} cites \
         this document as evidence but does not declare it in \"documents\", so \
         nothing signs it and nothing can be stated on it"
    )]
    DanglingEvidence {
        /// The `doc_id` as cited. Nothing in the dataset declares it.
        doc_id: String,
        /// The record field that cites it.
        cited_by: String,
    },
}

/// Where one failure belongs.
///
/// This is the single definition of "a fault of the dataset rather than of one
/// document", so a reader that degrades per document and a reader that refuses
/// the whole dataset cannot drift apart about which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureScope<'a> {
    /// A verdict on one `doc_id` the dataset rests on: one it declares, or
    /// one it cites as evidence without declaring. Both leave a single id to
    /// mark, which is what makes them per-document rather than dataset-level
    /// — a reader that degrades per document drops exactly that id's facts
    /// and keeps answering from the rest.
    Document {
        /// The document it is a verdict on.
        doc_id: &'a str,
        /// What kind of verdict it is.
        fault: DocumentFault,
    },
    /// A fault of the dataset as a whole: a repeated entry, a header this
    /// verifier cannot act on, a file nobody signed, a signature for a
    /// document the dataset does not declare. No per-document status can
    /// carry any of them.
    Dataset {
        /// What kind of dataset-level fault it is.
        fault: DatasetFault,
    },
}

/// What went wrong with one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFault {
    /// The document is there, and it is not the document that was signed.
    Integrity,
    /// There is no signed document to check.
    Absent,
}

/// What went wrong with the dataset as a whole.
///
/// A consumer that must tell a caller *that* a dataset is unusable without
/// telling them *which* file broke it reports [`DatasetFault::label`] and a
/// count. The kind lives here, beside the scope rule that decides which
/// failures are dataset-level at all, so the two cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetFault {
    /// A file is in the dataset that the manifest does not sign.
    UnsignedFile,
    /// The manifest signs a document `project.json` does not declare.
    UndeclaredDocument,
    /// The manifest as a set is not one a reader can act on: a repeated
    /// entry, a header for a scheme this verifier does not implement, an
    /// issuer listed twice.
    ManifestSet,
}

impl DatasetFault {
    /// What kind of fault this is, in words that name no file, no document
    /// and no issuer.
    ///
    /// This is the whole of what a caller who may not read the dataset's
    /// documents is told: naming the file, even to say it broke the dataset,
    /// would tell them a document exists that they may not see.
    pub const fn label(self) -> &'static str {
        match self {
            DatasetFault::UnsignedFile => "a file in the dataset that nothing signs",
            DatasetFault::UndeclaredDocument => "a signed document the dataset does not declare",
            DatasetFault::ManifestSet => "a manifest this verifier cannot act on",
        }
    }
}

impl CheckFailure {
    /// Where this failure belongs, and — for a document — what kind it is.
    ///
    /// Exhaustive on purpose: a new kind of failure must be placed here
    /// deliberately rather than fall into a catch-all that reports a broken
    /// dataset as sound.
    pub fn scope(&self) -> FailureScope<'_> {
        match self {
            CheckFailure::Hash { doc_id, .. }
            | CheckFailure::Signature { doc_id, .. }
            | CheckFailure::UnknownKey { doc_id, .. }
            | CheckFailure::Malformed { doc_id, .. } => FailureScope::Document {
                doc_id,
                fault: DocumentFault::Integrity,
            },
            CheckFailure::MissingFile { doc_id, .. }
            | CheckFailure::MissingFromManifest { doc_id, .. }
            | CheckFailure::DanglingEvidence { doc_id, .. } => FailureScope::Document {
                doc_id,
                fault: DocumentFault::Absent,
            },
            CheckFailure::UnsignedFile { .. } => FailureScope::Dataset {
                fault: DatasetFault::UnsignedFile,
            },
            CheckFailure::NotInProject { .. } => FailureScope::Dataset {
                fault: DatasetFault::UndeclaredDocument,
            },
            CheckFailure::Header { .. } => FailureScope::Dataset {
                fault: DatasetFault::ManifestSet,
            },
        }
    }
}

/// What a verification run found, and the bytes it found it in.
pub struct VerifyOutcome {
    pub dataset: String,
    /// Documents declared by `project.json` that passed every check.
    pub verified: usize,
    /// Distinct issuers across the manifest.
    pub issuers: usize,
    pub failures: Vec<CheckFailure>,
    /// The dataset description, validated from [`VerifyOutcome::project_json`].
    pub project: Project,
    /// The manifest as read in this run.
    pub manifest: Manifest,
    /// The exact `project.json` text this run hashed.
    ///
    /// A caller that needs more out of the record than the signing tools do —
    /// the server reads milestones, payments and the escrow balance — parses
    /// *this*. Re-opening the path instead would parse bytes that were never
    /// hashed, which is the whole window a tamper fits through.
    pub project_json: String,
}

impl VerifyOutcome {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }

    /// Documents declared by `project.json`.
    pub fn declared(&self) -> usize {
        self.project.documents.len()
    }

    /// Every failure that belongs to the dataset rather than to one document,
    /// each with its kind and its full text.
    ///
    /// These are the faults that leave no document to mark: a reader cannot
    /// answer "which of the two SR-05 entries is signed?" by degrading one
    /// document, so a consumer that answers from this dataset at all is
    /// answering from one its own verifier rejects. The kind comes back
    /// beside the text so a consumer can report *what kind* of fault it hit
    /// to a caller it may not show the text to.
    pub fn dataset_faults(&self) -> Vec<(DatasetFault, String)> {
        self.failures
            .iter()
            .filter_map(|failure| match failure.scope() {
                FailureScope::Dataset { fault } => Some((fault, failure.to_string())),
                FailureScope::Document { .. } => None,
            })
            .collect()
    }
}

/// Verify a dataset against its manifest and the committed issuer public keys.
///
/// `Err` is reserved for the case where verification cannot start at all — an
/// unreadable `project.json`, `manifest.json` or `issuers.json`. Everything a
/// verifier is supposed to catch comes back in `failures`.
///
/// `project.json` is read **once**. Its hash is computed over that buffer, and
/// the buffer itself comes back in the outcome, so the bytes that were checked
/// are the bytes a caller goes on to read.
pub fn verify_dataset(dataset_dir: &Path, keys_dir: &Path) -> Result<VerifyOutcome, DatasetError> {
    let dataset = dataset_name(dataset_dir)?;
    let project_path = dataset_dir.join(PROJECT_FILE);
    let project_json = read_capped(&project_path, "the dataset description", JSON_READ_LIMIT)?;
    let project_sha256 = sha256_bytes(project_json.as_bytes());
    let project = Project::from_text(&project_json, &display_path(&project_path))?;
    let manifest = Manifest::load(dataset_dir)?;
    let issuer_keys = IssuerKeys::load(keys_dir)?;

    // Three sets of manifest-level defects, all reported as `Header` because
    // none of them belongs to a single document: a header that describes a
    // scheme this verifier does not implement, an entry set that is not one a
    // reader can act on (a repeated key, or a manifest that leaves
    // `project.json` unsigned), and a trust anchor that lists one issuer
    // twice.
    let mut failures: Vec<CheckFailure> = manifest
        .header_problems(&dataset)
        .into_iter()
        .chain(manifest.structural_problems())
        .chain(issuer_keys.duplicate_problems())
        .map(|problem| CheckFailure::Header { problem })
        .collect();

    let mut verified = 0usize;
    for document in &project.documents {
        let Some(entry) = manifest.document(&document.doc_id) else {
            failures.push(CheckFailure::MissingFromManifest {
                doc_id: document.doc_id.clone(),
                path: document.path.clone(),
            });
            continue;
        };

        let mut document_failures = Vec::new();

        if entry.path != document.path {
            document_failures.push(CheckFailure::Malformed {
                doc_id: entry.doc_id.clone(),
                path: entry.path.clone(),
                problem: format!(
                    "the manifest records path {:?} but project.json declares {:?}",
                    entry.path, document.path
                ),
            });
        }
        if entry.issuer != document.issuer {
            document_failures.push(CheckFailure::Malformed {
                doc_id: entry.doc_id.clone(),
                path: entry.path.clone(),
                problem: format!(
                    "the manifest records issuer {:?} but project.json declares {:?}",
                    entry.issuer, document.issuer
                ),
            });
        }

        // `project.json` is hashed from the buffer read at the top of this
        // function, which is the buffer the caller parses. Every other
        // document is streamed off disk: nothing reads their contents again,
        // so the hash and the status it produces describe the same read.
        if entry.path == PROJECT_FILE {
            if project_sha256 != entry.sha256 {
                document_failures.push(CheckFailure::Hash {
                    doc_id: entry.doc_id.clone(),
                    path: entry.path.clone(),
                    recorded: entry.sha256.clone(),
                    actual: project_sha256.clone(),
                });
            }
        } else {
            let on_disk = Project::document_path(dataset_dir, &entry.path);
            if on_disk.exists() {
                let actual = sha256_file(&on_disk)?;
                if actual != entry.sha256 {
                    document_failures.push(CheckFailure::Hash {
                        doc_id: entry.doc_id.clone(),
                        path: entry.path.clone(),
                        recorded: entry.sha256.clone(),
                        actual,
                    });
                }
            } else {
                document_failures.push(CheckFailure::MissingFile {
                    doc_id: entry.doc_id.clone(),
                    path: entry.path.clone(),
                });
            }
        }

        // The cross-check. A signature only means something if the key that
        // made it is the key this issuer is on record as using.
        match issuer_keys.public_key(&entry.issuer) {
            None => document_failures.push(CheckFailure::UnknownKey {
                doc_id: entry.doc_id.clone(),
                path: entry.path.clone(),
                issuer: entry.issuer.clone(),
                manifest_key: entry.issuer_public_key.clone(),
                recorded: format!("issuer \"{}\" is not listed there at all", entry.issuer),
            }),
            Some(recorded) if recorded != entry.issuer_public_key => {
                document_failures.push(CheckFailure::UnknownKey {
                    doc_id: entry.doc_id.clone(),
                    path: entry.path.clone(),
                    issuer: entry.issuer.clone(),
                    manifest_key: entry.issuer_public_key.clone(),
                    recorded: format!("that issuer's recorded key is {recorded}"),
                });
            }
            Some(_) => {}
        }

        // Verified over the manifest's own fields, which is what was signed.
        // A tampered document therefore reports a hash failure, and a tampered
        // manifest reports a signature failure, instead of one masking the other.
        let payload = signing_payload(
            &manifest.dataset,
            &entry.doc_id,
            &entry.issuer,
            &entry.path,
            &entry.sha256,
        );
        match verify_payload(&entry.issuer_public_key, &payload, &entry.signature) {
            Ok(true) => {}
            Ok(false) => document_failures.push(CheckFailure::Signature {
                doc_id: entry.doc_id.clone(),
                path: entry.path.clone(),
                issuer: entry.issuer.clone(),
            }),
            Err(error) => document_failures.push(CheckFailure::Malformed {
                doc_id: entry.doc_id.clone(),
                path: entry.path.clone(),
                problem: error.to_string(),
            }),
        }

        if document_failures.is_empty() {
            verified += 1;
        } else {
            failures.append(&mut document_failures);
        }
    }

    let declared_ids: BTreeSet<&str> = project
        .documents
        .iter()
        .map(|document| document.doc_id.as_str())
        .collect();
    for entry in &manifest.documents {
        if !declared_ids.contains(entry.doc_id.as_str()) {
            failures.push(CheckFailure::NotInProject {
                doc_id: entry.doc_id.clone(),
                path: entry.path.clone(),
            });
        }
    }

    // The record's own citations, against the same declared set. A `doc_id`
    // cited as evidence but never declared is signed by nothing: there is no
    // path to hash, no issuer and no signature, so every fact resting on it
    // rests on bytes nobody stood behind. Nothing else in this run looks at
    // the record's body, which is why no gate caught this before.
    for reference in &project.evidence_references {
        if !declared_ids.contains(reference.doc_id.as_str()) {
            failures.push(CheckFailure::DanglingEvidence {
                doc_id: reference.doc_id.clone(),
                cited_by: reference.cited_by.clone(),
            });
        }
    }

    let signed_paths: BTreeSet<&str> = manifest
        .documents
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    for path in list_files_relative(dataset_dir, &dataset_dir.join(DOCS_DIR))? {
        if !signed_paths.contains(path.as_str()) {
            failures.push(CheckFailure::UnsignedFile { path });
        }
    }

    let issuers: BTreeSet<&str> = manifest
        .documents
        .iter()
        .map(|entry| entry.issuer.as_str())
        .collect();

    Ok(VerifyOutcome {
        dataset,
        verified,
        issuers: issuers.len(),
        failures,
        project,
        manifest,
        project_json,
    })
}
