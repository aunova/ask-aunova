//! End-to-end runs of the real `verify-dataset` binary: exit 0 on a clean
//! dataset, non-zero naming the document when one byte changes.
//!
//! The fixture is built inside the process temp directory (`TMPDIR`, pointed
//! at the repo's `.qa/tmp`). Never the real `data/palm-vista`.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dataset_manifest::manifest::Manifest;
use dataset_manifest::sign::sign_dataset;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const TAMPERED_DOC_ID: &str = "SR-02";
const TAMPERED_PATH: &str = "docs/site-progress-report-SR-02.md";

/// A dataset that covers its own record: `project.json` appears in its own
/// `documents`, so editing it is caught like any other document.
const PROJECT_JSON: &str = r#"{
  "watermark": "SYNTHETIC DEMO DATA",
  "issuers": [
    { "id": "quillfield-consulting-engineers", "name": "Quillfield Consulting Engineers" },
    { "id": "harborstone-escrow-trust", "name": "Harborstone Escrow Trust" },
    { "id": "lanternbay-developments", "name": "Lanternbay Developments" }
  ],
  "documents": [
    { "doc_id": "PROJECT-RECORD", "title": "Fixture project record",
      "path": "project.json",
      "issuer": "lanternbay-developments", "visibility": "public" },
    { "doc_id": "SR-01", "title": "Site progress report SR-01",
      "path": "docs/site-progress-report-SR-01.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    { "doc_id": "SR-02", "title": "Site progress report SR-02",
      "path": "docs/site-progress-report-SR-02.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    { "doc_id": "ESC-01", "title": "Escrow attestation ESC-01",
      "path": "docs/escrow-attestation-ESC-01.md",
      "issuer": "harborstone-escrow-trust", "visibility": "public" }
  ]
}
"#;

/// The same dataset with its own record left out of `documents` — the state
/// review finding F1 describes.
const PROJECT_JSON_WITHOUT_RECORD: &str = r#"{
  "watermark": "SYNTHETIC DEMO DATA",
  "issuers": [
    { "id": "quillfield-consulting-engineers", "name": "Quillfield Consulting Engineers" },
    { "id": "harborstone-escrow-trust", "name": "Harborstone Escrow Trust" },
    { "id": "lanternbay-developments", "name": "Lanternbay Developments" }
  ],
  "documents": [
    { "doc_id": "SR-01", "title": "Site progress report SR-01",
      "path": "docs/site-progress-report-SR-01.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    { "doc_id": "SR-02", "title": "Site progress report SR-02",
      "path": "docs/site-progress-report-SR-02.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    { "doc_id": "ESC-01", "title": "Escrow attestation ESC-01",
      "path": "docs/escrow-attestation-ESC-01.md",
      "issuer": "harborstone-escrow-trust", "visibility": "public" }
  ]
}
"#;

/// The same dataset whose record cites `SR-99` as the evidence behind a
/// milestone. Nothing in the dataset declares `SR-99`, so nothing signs it.
const PROJECT_JSON_CITING_UNDECLARED: &str = r#"{
  "watermark": "SYNTHETIC DEMO DATA",
  "issuers": [
    { "id": "quillfield-consulting-engineers", "name": "Quillfield Consulting Engineers" },
    { "id": "harborstone-escrow-trust", "name": "Harborstone Escrow Trust" },
    { "id": "lanternbay-developments", "name": "Lanternbay Developments" }
  ],
  "documents": [
    { "doc_id": "PROJECT-RECORD", "title": "Fixture project record",
      "path": "project.json",
      "issuer": "lanternbay-developments", "visibility": "public" },
    { "doc_id": "SR-01", "title": "Site progress report SR-01",
      "path": "docs/site-progress-report-SR-01.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    { "doc_id": "SR-02", "title": "Site progress report SR-02",
      "path": "docs/site-progress-report-SR-02.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    { "doc_id": "ESC-01", "title": "Escrow attestation ESC-01",
      "path": "docs/escrow-attestation-ESC-01.md",
      "issuer": "harborstone-escrow-trust", "visibility": "public" }
  ],
  "milestones": [
    { "id": "MS-01", "title": "Structure topped out", "evidence": ["SR-99"],
      "visibility": "public" }
  ]
}
"#;

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    /// Build and sign a fixture dataset.
    fn signed(label: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ask-aunova-verify-cli-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        let dataset_dir = root.join("data").join("palm-vista");
        let docs = dataset_dir.join("docs");
        std::fs::create_dir_all(&docs).expect("create the scratch dataset");
        std::fs::write(dataset_dir.join("project.json"), PROJECT_JSON).expect("write project.json");
        for name in [
            "site-progress-report-SR-01.md",
            "site-progress-report-SR-02.md",
            "escrow-attestation-ESC-01.md",
        ] {
            std::fs::write(
                docs.join(name),
                format!("# SYNTHETIC DEMO DATA\n\nFictional document {name}.\n"),
            )
            .expect("write a fixture document");
        }
        let scratch = Scratch { root };
        sign_dataset(&scratch.dataset_dir(), &scratch.root.join("keys"), true)
            .expect("sign the fixture");
        scratch
    }

    fn dataset_dir(&self) -> PathBuf {
        self.root.join("data").join("palm-vista")
    }

    /// Run the real binary with the scratch root as its working directory, so
    /// the default `data/palm-vista` and `keys` arguments are exercised.
    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_verify-dataset"))
            .current_dir(&self.root)
            .output()
            .expect("run verify-dataset")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_clean_dataset_exits_zero_with_a_summary() {
    let scratch = Scratch::signed("clean");
    let output = scratch.run();
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert_eq!(
        stdout(&output).trim(),
        "verified 4 documents from 3 issuers in dataset \"palm-vista\""
    );
}

#[test]
fn one_flipped_byte_exits_non_zero_and_names_the_document_and_its_path() {
    let scratch = Scratch::signed("flipped");
    let path = scratch.dataset_dir().join(TAMPERED_PATH);
    let mut bytes = std::fs::read(&path).expect("read a fixture document");
    bytes[0] ^= 0x20;
    std::fs::write(&path, bytes).expect("write a tampered fixture document");

    let output = scratch.run();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("hash:"), "{message}");
    assert!(message.contains(TAMPERED_DOC_ID), "{message}");
    assert!(message.contains(TAMPERED_PATH), "{message}");
    assert!(
        message.contains("3 of 4 declared documents verified"),
        "{message}"
    );
}

#[test]
fn a_missing_manifest_is_reported_rather_than_treated_as_clean() {
    let scratch = Scratch::signed("no-manifest");
    std::fs::remove_file(Manifest::path_in(&scratch.dataset_dir())).expect("remove the manifest");

    let output = scratch.run();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("manifest.json"), "{message}");
    assert!(message.contains("the dataset manifest"), "{message}");
}

/// Review finding F1, case D1: the attacker rewrites `project.json` and
/// deletes its entry from both files. Every remaining signature still checks
/// out, and the dataset must still be refused.
#[test]
fn a_manifest_that_covers_no_record_exits_non_zero() {
    let scratch = Scratch::signed("no-record");
    std::fs::write(
        scratch.dataset_dir().join("project.json"),
        PROJECT_JSON_WITHOUT_RECORD,
    )
    .expect("rewrite project.json without its own record");
    let mut manifest = Manifest::load(&scratch.dataset_dir()).expect("load the fixture manifest");
    manifest
        .documents
        .retain(|entry| entry.path != "project.json");
    manifest
        .write(&scratch.dataset_dir())
        .expect("write the stripped manifest");

    let output = scratch.run();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("manifest header:"), "{message}");
    assert!(message.contains("project.json"), "{message}");
}

/// Review finding F3, reproduced end to end: a second entry for one `doc_id`,
/// with a hash of 64 zeroes and a signature of 128 ones. This exited 0 before.
#[test]
fn a_duplicate_manifest_entry_exits_non_zero() {
    let scratch = Scratch::signed("duplicate");
    let mut manifest = Manifest::load(&scratch.dataset_dir()).expect("load the fixture manifest");
    let mut bogus = manifest
        .documents
        .iter()
        .find(|entry| entry.doc_id == TAMPERED_DOC_ID)
        .expect("the fixture signs SR-02")
        .clone();
    bogus.sha256 = "0".repeat(64);
    bogus.signature = "1".repeat(128);
    manifest.documents.push(bogus);
    manifest
        .write(&scratch.dataset_dir())
        .expect("write the manifest with a duplicate entry");

    let output = scratch.run();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains(TAMPERED_DOC_ID), "{message}");
    assert!(message.contains("2 times"), "{message}");
}

/// The r4 follow-on, end to end: the record cites a document the dataset does
/// not declare. Every hash and signature still checks out and every file under
/// `docs/` is signed, so before this check the binary exited 0 on it — which
/// is what made the server the only thing standing between a dangling citation
/// and a caller.
#[test]
fn a_record_citing_an_undeclared_document_exits_non_zero() {
    let scratch = Scratch::signed("dangling-evidence");
    std::fs::write(
        scratch.dataset_dir().join("project.json"),
        PROJECT_JSON_CITING_UNDECLARED,
    )
    .expect("rewrite project.json with a citation of an undeclared document");
    // Re-signed, so the record's own signature is sound: the only thing wrong
    // with this dataset is the cross-reference inside it.
    sign_dataset(&scratch.dataset_dir(), &scratch.root.join("keys"), true)
        .expect("re-sign the rewritten record");

    let output = scratch.run();
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("dangling evidence:"), "{message}");
    assert!(message.contains("SR-99"), "{message}");
    assert!(message.contains("MS-01"), "{message}");
    assert!(message.contains("\"evidence\""), "{message}");
}
