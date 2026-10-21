//! `keys/public/issuers.json` — the committed list of issuer public keys.
//!
//! This file is what stops a manifest from proving nothing. Anyone who can edit
//! `manifest.json` can re-sign every entry with a key they made up; the only
//! defence is an independently committed record of which public key each issuer
//! actually uses, which is this file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DatasetError, display_path};
use crate::files::{JSON_READ_LIMIT, read_capped};
use crate::project::SYNTHETIC_WATERMARK;
use crate::structure::duplicates;

/// Subdirectory of the key directory holding the committed public keys.
pub const PUBLIC_SUBDIR: &str = "public";

/// Name of the public key list inside the public key directory.
pub const ISSUERS_FILE: &str = "issuers.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicIssuer {
    pub id: String,
    pub name: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuerKeys {
    pub watermark: String,
    pub issuers: Vec<PublicIssuer>,
}

impl IssuerKeys {
    /// Build the list, sorted by issuer id so the committed file is
    /// deterministic.
    pub fn new(mut issuers: Vec<PublicIssuer>) -> Self {
        issuers.sort_by(|left, right| left.id.cmp(&right.id));
        IssuerKeys {
            watermark: SYNTHETIC_WATERMARK.to_string(),
            issuers,
        }
    }

    pub fn path_in(keys_dir: &Path) -> PathBuf {
        keys_dir.join(PUBLIC_SUBDIR).join(ISSUERS_FILE)
    }

    pub fn load(keys_dir: &Path) -> Result<Self, DatasetError> {
        let path = Self::path_in(keys_dir);
        let text = read_capped(&path, "the issuer public key list", JSON_READ_LIMIT)?;
        serde_json::from_str(&text).map_err(|source| DatasetError::Json {
            what: "the issuer public key list",
            path: display_path(&path),
            source,
        })
    }

    pub fn write(&self, keys_dir: &Path) -> Result<PathBuf, DatasetError> {
        let path = Self::path_in(keys_dir);
        let directory = keys_dir.join(PUBLIC_SUBDIR);
        std::fs::create_dir_all(&directory).map_err(|source| DatasetError::CreateDir {
            what: "the issuer public key list",
            path: display_path(&directory),
            source,
        })?;
        let mut json = serde_json::to_string_pretty(self).map_err(|source| DatasetError::Json {
            what: "the issuer public key list",
            path: display_path(&path),
            source,
        })?;
        json.push('\n');
        std::fs::write(&path, json).map_err(|source| DatasetError::Write {
            what: "the issuer public key list",
            path: display_path(&path),
            source,
        })?;
        Ok(path)
    }

    pub fn public_key(&self, issuer_id: &str) -> Option<&str> {
        self.issuers
            .iter()
            .find(|issuer| issuer.id == issuer_id)
            .map(|issuer| issuer.public_key.as_str())
    }

    /// Every issuer id this list declares more than once, as human-readable
    /// lines.
    ///
    /// [`IssuerKeys::public_key`] takes the first match, so a second entry for
    /// one issuer is a second answer to "which key is this issuer on record as
    /// using" — and the trust anchor of the whole scheme cannot have two
    /// answers.
    pub fn duplicate_problems(&self) -> Vec<String> {
        duplicates(
            self.issuers.iter().map(|issuer| issuer.id.as_str()),
            &format!("the issuer public key list {PUBLIC_SUBDIR}/{ISSUERS_FILE}"),
            "id",
        )
    }
}
