//! A dataset fixture of its own for every test that needs to break something.
//!
//! Never the committed `data/palm-vista`: a test that tampers with a document
//! copies the signed dataset into `TMPDIR` first and points the server at the
//! copy, so the tracked dataset is only ever read.
//!
//! The copy keeps the dataset's own name. A dataset's name is part of every
//! signing payload and is checked against the directory it is verified in, so
//! a fixture copied into a directory called anything else is a dataset
//! `verify-dataset` rejects — and a test running against one would be proving
//! the server's behaviour on a dataset it must refuse.

// Rust compiles this module separately into each test binary, and each one
// uses the part of it that its own tests need: `mcp_over_http` tampers and
// duplicates entries, `signed_record` only copies. Without this the unused
// half is a dead-code warning in whichever binary does not call it.
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use ask_aunova_server::verify::DatasetConfig;

/// Makes each fixture directory unique within one test binary run.
static NEXT_FIXTURE: AtomicU32 = AtomicU32::new(0);

/// The committed dataset every fixture is a copy of, relative to the
/// repository root.
const DATASET: &str = "data/palm-vista";

/// The name the copy must keep: the manifest signs it, and verification
/// checks it against the directory the dataset is verified in.
const DATASET_NAME: &str = "palm-vista";

/// A throwaway copy of the signed dataset, removed when the test ends.
pub struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// Copy the committed dataset and the public issuer keys into `TMPDIR`.
    pub fn new(name: &str) -> Self {
        let ordinal = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ask-aunova-{name}-{}-{ordinal}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("clear a stale fixture directory");
        }
        fs::create_dir_all(&root).expect("create the fixture directory");

        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the server package sits inside the repository")
            .to_path_buf();
        copy_tree(&repo.join(DATASET), &root.join(DATASET_NAME));
        fs::create_dir_all(root.join("keys/public")).expect("create the fixture keys directory");
        fs::copy(
            repo.join("keys/public/issuers.json"),
            root.join("keys/public/issuers.json"),
        )
        .expect("copy the committed issuer public keys");

        Self { root }
    }

    /// The dataset directory inside the fixture.
    pub fn dataset_dir(&self) -> PathBuf {
        self.root.join(DATASET_NAME)
    }

    /// What the server should be configured with.
    pub fn config(&self) -> DatasetConfig {
        DatasetConfig::new(self.dataset_dir(), self.root.join("keys"))
    }

    /// Flip one bit of one byte of a file inside the fixture dataset.
    ///
    /// One bit is the whole point: the document still reads as the same
    /// document to a human, and the hash no longer matches what was signed.
    pub fn tamper(&self, relative_path: &str) {
        let path = self.dataset_dir().join(relative_path);
        let mut bytes = fs::read(&path).unwrap_or_else(|error| {
            panic!("read {} from the fixture: {error}", path.display());
        });
        let last = bytes
            .len()
            .checked_sub(1)
            .expect("the document is not empty");
        bytes[last] ^= 0x01;
        fs::write(&path, bytes).unwrap_or_else(|error| {
            panic!("write {} back into the fixture: {error}", path.display());
        });
    }

    /// Replace one occurrence of `from` with `to` inside a fixture file.
    ///
    /// Used where the tampered file still has to parse — editing a number in
    /// `project.json` is the on-camera demo, and it is a different failure
    /// from a file that stops being JSON at all.
    pub fn replace_once(&self, relative_path: &str, from: &str, to: &str) {
        let path = self.dataset_dir().join(relative_path);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert_eq!(
            text.matches(from).count(),
            1,
            "`{from}` must appear exactly once in {relative_path} for this edit to mean \
             anything",
        );
        fs::write(&path, text.replacen(from, to, 1))
            .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }

    /// Make the record cite `doc_id` as evidence for `milestone_id` while
    /// declaring no such document.
    ///
    /// The declaration is dropped from `documents` and the file it named is
    /// deleted, so nothing is left under `docs/` for the manifest to sign and
    /// the dataset is sound in every other respect. The id is added to
    /// `milestone_id`'s `evidence`, which is the list every caller's answer
    /// resolves — a `contractor_note` would not do, because that one is
    /// filtered by visibility before it is ever resolved.
    ///
    /// The record changes, so the caller re-signs after this.
    pub fn cite_without_declaring(&self, doc_id: &str, milestone_id: &str) {
        let path = self.dataset_dir().join("project.json");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let mut record: serde_json::Value =
            serde_json::from_str(&text).expect("the fixture record parses");

        let documents = record["documents"]
            .as_array_mut()
            .expect("the record declares documents");
        let position = documents
            .iter()
            .position(|document| document["doc_id"] == doc_id)
            .unwrap_or_else(|| panic!("the record declares no `{doc_id}`"));
        let declared = documents.remove(position);
        let document_path = declared["path"]
            .as_str()
            .unwrap_or_else(|| panic!("`{doc_id}` is declared without a path"));
        let document_path = self.dataset_dir().join(document_path);

        let milestones = record["milestones"]
            .as_array_mut()
            .expect("the record carries milestones");
        let milestone = milestones
            .iter_mut()
            .find(|milestone| milestone["id"] == milestone_id)
            .unwrap_or_else(|| panic!("the record carries no `{milestone_id}`"));
        milestone["evidence"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("`{milestone_id}` carries no evidence list"))
            .push(serde_json::Value::String(doc_id.to_owned()));

        let mut json = serde_json::to_string_pretty(&record).expect("the record re-serialises");
        json.push('\n');
        fs::write(&path, json).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
        fs::remove_file(&document_path).unwrap_or_else(|error| {
            panic!(
                "remove {} from the fixture: {error}",
                document_path.display()
            )
        });
    }

    /// Repeat one manifest entry, so the dataset carries the set-level defect
    /// `verify-dataset` refuses: one `doc_id` signed twice.
    ///
    /// The copy is byte-identical to the entry it duplicates, so every hash
    /// and signature in the dataset still checks out. What is broken is the
    /// manifest as a set, which is exactly the fault no per-document status
    /// can carry.
    pub fn duplicate_manifest_entry(&self, doc_id: &str) {
        let path = self.dataset_dir().join("manifest.json");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let mut manifest: serde_json::Value =
            serde_json::from_str(&text).expect("the fixture manifest parses");
        let entries = manifest["documents"]
            .as_array_mut()
            .expect("the manifest lists documents");
        let entry = entries
            .iter()
            .find(|entry| entry["doc_id"] == doc_id)
            .unwrap_or_else(|| panic!("the manifest signs no `{doc_id}`"))
            .clone();
        entries.push(entry);
        let mut json = serde_json::to_string_pretty(&manifest).expect("the manifest re-serialises");
        json.push('\n');
        fs::write(&path, json).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }

    /// Drop one document's entry from the manifest, leaving the file in the
    /// dataset with nothing signing it.
    ///
    /// This is a fault of the dataset as a whole, not of one document: the
    /// file is present, every hash and signature that remains still checks
    /// out, and there is no document to mark — which is why a reader must
    /// refuse rather than degrade. The refusal is the one the r3 review found
    /// naming `docs/contractor-note-CN-14.md` to a buyer.
    pub fn remove_manifest_entry(&self, doc_id: &str) {
        let path = self.dataset_dir().join("manifest.json");
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let mut manifest: serde_json::Value =
            serde_json::from_str(&text).expect("the fixture manifest parses");
        let entries = manifest["documents"]
            .as_array_mut()
            .expect("the manifest lists documents");
        let before = entries.len();
        entries.retain(|entry| entry["doc_id"] != doc_id);
        assert_eq!(
            entries.len() + 1,
            before,
            "the manifest must sign `{doc_id}` exactly once for this edit to mean anything",
        );
        let mut json = serde_json::to_string_pretty(&manifest).expect("the manifest re-serialises");
        json.push('\n');
        fs::write(&path, json).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }

    /// Re-sign the fixture dataset after its record has been edited.
    ///
    /// Keys are generated into the fixture, never read from the repository:
    /// a test that needed the committed private keys would be a test that
    /// could sign something the demo trusts. The fixture's own
    /// `keys/public/issuers.json` is rewritten to the keys that just signed,
    /// so the server verifies the copy against the copy.
    pub fn re_sign(&self) {
        let keys_dir = self.root.join("keys");
        let outcome = dataset_manifest::sign::sign_dataset(&self.dataset_dir(), &keys_dir, true)
            .unwrap_or_else(|error| panic!("re-sign the fixture dataset: {error}"));
        assert!(
            outcome.documents > 0,
            "a re-signed fixture must sign the documents it declares",
        );
    }

    /// Re-hash and re-sign ONE manifest entry from the file's raw bytes, never parsing it.
    ///
    /// `re_sign` now refuses a record that does not parse (the dangling-evidence
    /// gate reads it), which is right for the tool and wrong for a test whose
    /// whole premise is that an issuer signed a malformed record anyway. Call
    /// `re_sign` first so the fixture holds its own private keys.
    pub fn re_sign_entry(&self, doc_id: &str) {
        use dataset_manifest::{canonical, files, keys, manifest::Manifest};
        let dataset_dir = self.dataset_dir();
        let keys_dir = self.root.join("keys");
        let mut manifest = Manifest::load(&dataset_dir).expect("the fixture manifest loads");
        let dataset = manifest.dataset.clone();
        let entry = manifest
            .documents
            .iter_mut()
            .find(|document| document.doc_id == doc_id)
            .unwrap_or_else(|| panic!("the fixture manifest has no entry {doc_id:?}"));
        let bytes = fs::read(dataset_dir.join(&entry.path)).expect("read the entry's file");
        entry.sha256 = files::sha256_bytes(&bytes);
        let signing_key = keys::load_signing_key(&keys_dir, &entry.issuer)
            .expect("the fixture holds the issuer key");
        let payload = canonical::signing_payload(
            &dataset,
            &entry.doc_id,
            &entry.issuer,
            &entry.path,
            &entry.sha256,
        );
        entry.signature =
            keys::sign_payload(&signing_key, &entry.doc_id, &payload).expect("sign the entry");
        manifest
            .write(&dataset_dir)
            .expect("write the fixture manifest");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Best effort: a leftover directory under TMPDIR fails no test, and
        // panicking in a drop while a test is already failing hides the
        // failure that matters.
        let _removed = fs::remove_dir_all(&self.root);
    }
}

/// Copy a directory tree, creating what it needs as it goes.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap_or_else(|error| panic!("create {}: {error}", to.display()));
    let entries = fs::read_dir(from)
        .unwrap_or_else(|error| panic!("read the directory {}: {error}", from.display()));
    for entry in entries {
        let entry = entry.expect("read a directory entry");
        let kind = entry.file_type().expect("read an entry's file type");
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&source, &destination);
        } else {
            fs::copy(&source, &destination)
                .unwrap_or_else(|error| panic!("copy {}: {error}", source.display()));
        }
    }
}
