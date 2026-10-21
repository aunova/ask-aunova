//! `manifest.json` — the signed record of a dataset.
//!
//! Written with `documents[]` sorted by `doc_id` and with no wall-clock
//! timestamp anywhere, so re-signing an unchanged dataset produces a
//! byte-identical file and any diff is an honest one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::canonical::SIGNING_PAYLOAD_DESCRIPTION;
use crate::error::{DatasetError, display_path};
use crate::files::{JSON_READ_LIMIT, read_capped};
use crate::project::SYNTHETIC_WATERMARK;
use crate::structure;

/// Name of the manifest inside a dataset directory.
pub const MANIFEST_FILE: &str = "manifest.json";

pub const HASH_ALGORITHM: &str = "sha256";
pub const SIGNATURE_ALGORITHM: &str = "ed25519";
pub const HEX_ENCODING: &str = "lowercase-hex";

/// What a verifier must implement to check this manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Algorithm {
    pub hash: String,
    pub signature: String,
    pub encoding: String,
}

/// One signed document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDocument {
    pub doc_id: String,
    pub path: String,
    pub sha256: String,
    pub issuer: String,
    pub issuer_public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub watermark: String,
    pub dataset: String,
    pub algorithm: Algorithm,
    pub signing_payload: String,
    pub documents: Vec<ManifestDocument>,
}

impl Manifest {
    /// Build a manifest, sorting `documents` so the output is deterministic.
    /// Sorting here rather than at the call site makes determinism structural
    /// instead of something a caller has to remember.
    pub fn new(dataset: &str, mut documents: Vec<ManifestDocument>) -> Self {
        documents.sort_by(|left, right| left.doc_id.cmp(&right.doc_id));
        Manifest {
            watermark: SYNTHETIC_WATERMARK.to_string(),
            dataset: dataset.to_string(),
            algorithm: Algorithm {
                hash: HASH_ALGORITHM.to_string(),
                signature: SIGNATURE_ALGORITHM.to_string(),
                encoding: HEX_ENCODING.to_string(),
            },
            signing_payload: SIGNING_PAYLOAD_DESCRIPTION.to_string(),
            documents,
        }
    }

    pub fn path_in(dataset_dir: &Path) -> PathBuf {
        dataset_dir.join(MANIFEST_FILE)
    }

    pub fn load(dataset_dir: &Path) -> Result<Self, DatasetError> {
        let path = Self::path_in(dataset_dir);
        let text = read_capped(&path, "the dataset manifest", JSON_READ_LIMIT)?;
        serde_json::from_str(&text).map_err(|source| DatasetError::Json {
            what: "the dataset manifest",
            path: display_path(&path),
            source,
        })
    }

    pub fn write(&self, dataset_dir: &Path) -> Result<PathBuf, DatasetError> {
        let path = Self::path_in(dataset_dir);
        let mut json = serde_json::to_string_pretty(self).map_err(|source| DatasetError::Json {
            what: "the dataset manifest",
            path: display_path(&path),
            source,
        })?;
        json.push('\n');
        std::fs::write(&path, json).map_err(|source| DatasetError::Write {
            what: "the dataset manifest",
            path: display_path(&path),
            source,
        })?;
        Ok(path)
    }

    pub fn document(&self, doc_id: &str) -> Option<&ManifestDocument> {
        self.documents
            .iter()
            .find(|document| document.doc_id == doc_id)
    }

    /// Everything structurally wrong with the set of entries, as
    /// human-readable lines: a `doc_id` or a `path` that appears twice, and a
    /// manifest that does not sign `project.json`.
    ///
    /// Separate from [`Manifest::header_problems`] because these are defects
    /// of the entry set rather than of the header, and because signing runs
    /// the same rule over the dataset's own `documents` before it writes
    /// anything.
    pub fn structural_problems(&self) -> Vec<String> {
        let entries: Vec<(&str, &str)> = self
            .documents
            .iter()
            .map(|document| (document.doc_id.as_str(), document.path.as_str()))
            .collect();
        structure::problems(&entries, "the manifest")
    }

    /// Everything wrong with the manifest's header, as human-readable lines.
    ///
    /// A manifest that names a different algorithm must not be checked with
    /// this one and silently reported as valid.
    pub fn header_problems(&self, dataset: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if self.watermark != SYNTHETIC_WATERMARK {
            problems.push(format!(
                "\"watermark\" is {:?}, expected {SYNTHETIC_WATERMARK:?}",
                self.watermark
            ));
        }
        if self.dataset != dataset {
            problems.push(format!(
                "\"dataset\" is {:?}, but the directory being verified is {dataset:?}",
                self.dataset
            ));
        }
        if self.algorithm.hash != HASH_ALGORITHM {
            problems.push(format!(
                "\"algorithm.hash\" is {:?}, and this verifier only implements {HASH_ALGORITHM:?}",
                self.algorithm.hash
            ));
        }
        if self.algorithm.signature != SIGNATURE_ALGORITHM {
            problems.push(format!(
                "\"algorithm.signature\" is {:?}, and this verifier only implements {SIGNATURE_ALGORITHM:?}",
                self.algorithm.signature
            ));
        }
        if self.algorithm.encoding != HEX_ENCODING {
            problems.push(format!(
                "\"algorithm.encoding\" is {:?}, and this verifier only implements {HEX_ENCODING:?}",
                self.algorithm.encoding
            ));
        }
        if self.signing_payload != SIGNING_PAYLOAD_DESCRIPTION {
            problems.push(
                "\"signing_payload\" does not describe the payload this verifier reconstructs"
                    .to_string(),
            );
        }
        problems
    }
}
