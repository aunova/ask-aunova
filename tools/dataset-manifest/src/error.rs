//! One typed error for everything the dataset tools can fail at.
//!
//! House rule: `thiserror` at the library boundary, and every message names
//! *what* was expected and *where*, so an operator does not have to guess which
//! file or which issuer went wrong. The binaries print the `Display`
//! chain and nothing else.

use std::path::Path;

use thiserror::Error;

/// Render a path for an error message. Paths handed to these tools are
/// repo-relative, so this stays repo-relative too.
pub(crate) fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[derive(Debug, Error)]
pub enum DatasetError {
    #[error("could not read {what} at {path}: {source}")]
    Read {
        what: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not write {what} at {path}: {source}")]
    Write {
        what: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not create the directory {path} for {what}: {source}")]
    CreateDir {
        what: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} is not valid JSON for {what}: {source}")]
    Json {
        what: &'static str,
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("{path} is larger than the {limit}-byte limit this tool reads for {what}")]
    TooLarge {
        what: &'static str,
        path: String,
        limit: usize,
    },

    #[error("{what} at {path} is not usable:\n  - {}", .problems.join("\n  - "))]
    Invalid {
        what: &'static str,
        path: String,
        problems: Vec<String>,
    },

    #[error(
        "no private key for issuer \"{issuer}\": expected {path}; \
         re-run with --generate-missing-keys to create one"
    )]
    MissingPrivateKey { issuer: String, path: String },

    #[error(
        "the private key for issuer \"{issuer}\" at {path} is malformed: expected {expected}, found {found}"
    )]
    MalformedPrivateKey {
        issuer: String,
        path: String,
        expected: String,
        found: String,
    },

    #[error(
        "could not read {bytes} bytes of operating-system entropy for a new issuer keypair: {source}"
    )]
    Entropy {
        bytes: usize,
        #[source]
        source: getrandom::Error,
    },

    #[error("could not produce an Ed25519 signature for {doc_id}: {detail}")]
    Sign { doc_id: String, detail: String },

    #[error("expected {expected}, found {found}")]
    MalformedHex { expected: String, found: String },

    #[error(
        "cannot derive a dataset name from {path}: pass a dataset directory \
         whose last path component is the dataset name, for example data/palm-vista"
    )]
    NamelessDataset { path: String },

    #[error(
        "refusing to write the private key {path}: owner-only (0600) file \
         permissions are only enforced on Unix, and this build is not Unix"
    )]
    UnsupportedKeyPermissions { path: String },
}
