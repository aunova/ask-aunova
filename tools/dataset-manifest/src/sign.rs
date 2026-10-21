//! Signing a dataset: hash every declared document, sign each one with its
//! issuer's key, and write `manifest.json` plus the committed public key list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;

use crate::canonical::signing_payload;
use crate::error::{DatasetError, display_path};
use crate::files::{list_files_relative, sha256_file};
use crate::issuers::{IssuerKeys, PublicIssuer};
use crate::keys::{
    generate_signing_key, load_signing_key, private_key_path, public_key_hex, sign_payload,
};
use crate::manifest::{Manifest, ManifestDocument};
use crate::project::{DOCS_DIR, Project, dataset_name};
use crate::structure;

/// What a signing run did.
#[derive(Debug)]
pub struct SignOutcome {
    pub dataset: String,
    pub documents: usize,
    pub issuers: usize,
    /// Issuer ids whose keypair this run created.
    pub generated_keys: Vec<String>,
    pub manifest_path: PathBuf,
    pub issuers_path: PathBuf,
}

/// Sign every document declared in `<dataset_dir>/project.json`.
///
/// Fails rather than writing anything — no manifest, and no generated key —
/// if a file sits under `docs/` without being declared, if two entries share a
/// `doc_id` or a `path`, or if the dataset does not declare `project.json`
/// itself. The first would ship an unsigned document; the second gives one id
/// or one file two signed records, so a reader and this verifier could
/// disagree about which one is real; the third leaves the record the whole
/// dataset answers from covered by no signature at all.
pub fn sign_dataset(
    dataset_dir: &Path,
    keys_dir: &Path,
    generate_missing_keys: bool,
) -> Result<SignOutcome, DatasetError> {
    let dataset = dataset_name(dataset_dir)?;
    let project = Project::load(dataset_dir)?;

    reject_undeclared_documents(dataset_dir, &project)?;
    reject_unsignable_document_set(dataset_dir, &project)?;

    let mut signing_keys: BTreeMap<String, SigningKey> = BTreeMap::new();
    let mut generated_keys = Vec::new();
    for issuer in &project.issuers {
        let key = if private_key_path(keys_dir, &issuer.id).exists() {
            load_signing_key(keys_dir, &issuer.id)?
        } else if generate_missing_keys {
            generated_keys.push(issuer.id.clone());
            generate_signing_key(keys_dir, &issuer.id)?
        } else {
            return Err(DatasetError::MissingPrivateKey {
                issuer: issuer.id.clone(),
                path: display_path(&private_key_path(keys_dir, &issuer.id)),
            });
        };
        signing_keys.insert(issuer.id.clone(), key);
    }

    let mut documents = Vec::with_capacity(project.documents.len());
    for document in &project.documents {
        let Some(signing_key) = signing_keys.get(&document.issuer) else {
            // Project validation already rejects an undeclared issuer, so this
            // only fires if that invariant is ever broken.
            return Err(DatasetError::Invalid {
                what: "the dataset description",
                path: display_path(dataset_dir),
                problems: vec![format!(
                    "document {:?} names issuer {:?}, for which no key was loaded",
                    document.doc_id, document.issuer
                )],
            });
        };
        let sha256 = sha256_file(&Project::document_path(dataset_dir, &document.path))?;
        let payload = signing_payload(
            &dataset,
            &document.doc_id,
            &document.issuer,
            &document.path,
            &sha256,
        );
        let signature = sign_payload(signing_key, &document.doc_id, &payload)?;
        documents.push(ManifestDocument {
            doc_id: document.doc_id.clone(),
            path: document.path.clone(),
            sha256,
            issuer: document.issuer.clone(),
            issuer_public_key: public_key_hex(signing_key),
            signature,
        });
    }

    let document_count = documents.len();
    let manifest_path = Manifest::new(&dataset, documents).write(dataset_dir)?;

    // Written on every run, not only when keys are generated, so the committed
    // public key list can never drift away from the keys that signed.
    let public_issuers = project
        .issuers
        .iter()
        .filter_map(|issuer| {
            signing_keys.get(&issuer.id).map(|key| PublicIssuer {
                id: issuer.id.clone(),
                name: issuer.name.clone(),
                public_key: public_key_hex(key),
            })
        })
        .collect();
    let issuers_path = IssuerKeys::new(public_issuers).write(keys_dir)?;

    Ok(SignOutcome {
        dataset,
        documents: document_count,
        issuers: project.issuers.len(),
        generated_keys,
        manifest_path,
        issuers_path,
    })
}

fn reject_undeclared_documents(dataset_dir: &Path, project: &Project) -> Result<(), DatasetError> {
    let declared: BTreeSet<&str> = project
        .documents
        .iter()
        .map(|document| document.path.as_str())
        .collect();
    let docs_dir = dataset_dir.join(DOCS_DIR);
    let present = list_files_relative(dataset_dir, &docs_dir)?;
    let problems: Vec<String> = present
        .into_iter()
        .filter(|path| !declared.contains(path.as_str()))
        .map(|path| {
            format!("{path} is present under {DOCS_DIR}/ but is not listed in \"documents\", so signing it is impossible and leaving it would ship an unsigned document")
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(DatasetError::Invalid {
            what: "the dataset",
            path: display_path(dataset_dir),
            problems,
        })
    }
}

/// Refuse a dataset whose declared set is not one a verifier would accept: a
/// repeated `doc_id` or `path`, or a dataset that does not declare its own
/// `project.json`.
///
/// The same rule the verifier runs over `manifest.json`, run here over the
/// `documents` the manifest is about to be built from, so signing cannot
/// produce a manifest its own verifier rejects. It runs before any key is
/// generated and before any file is written, so a refusal leaves the working
/// tree exactly as it was.
fn reject_unsignable_document_set(
    dataset_dir: &Path,
    project: &Project,
) -> Result<(), DatasetError> {
    let entries: Vec<(&str, &str)> = project
        .documents
        .iter()
        .map(|document| (document.doc_id.as_str(), document.path.as_str()))
        .collect();
    let problems = structure::problems(&entries, "the dataset's \"documents\"");
    if problems.is_empty() {
        Ok(())
    } else {
        Err(DatasetError::Invalid {
            what: "the dataset",
            path: display_path(dataset_dir),
            problems,
        })
    }
}
