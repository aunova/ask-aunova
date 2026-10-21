//! A throwaway dataset built inside the process temp directory (`TMPDIR`,
//! which this repo points at `.qa/tmp`). Never the real `data/palm-vista`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dataset_manifest::manifest::Manifest;
use dataset_manifest::project::Project;
use dataset_manifest::sign::sign_dataset;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub const ISSUER_A: &str = "quillfield-consulting-engineers";
pub const ISSUER_B: &str = "harborstone-escrow-trust";
pub const ISSUER_C: &str = "lanternbay-developments";

/// The dataset's own record, declared among its documents so that editing
/// `project.json` is caught. The fixtures differ only in this entry, so it is
/// spliced in rather than copied into three near-identical documents.
pub const PROJECT_RECORD_ENTRY: &str = r#"{ "doc_id": "PROJECT-RECORD", "title": "Fixture project record",
      "path": "project.json",
      "issuer": "lanternbay-developments", "visibility": "public" },
    "#;

/// The same record under another `doc_id`: a dataset is free to name it what
/// it likes, because what is signed is the path and the content hash.
pub const RENAMED_RECORD_ENTRY: &str = r#"{ "doc_id": "DATASET-RECORD", "title": "Fixture project record",
      "path": "project.json",
      "issuer": "lanternbay-developments", "visibility": "public" },
    "#;

/// A dataset that does not cover its own record — the hole review finding F1
/// names.
pub const NO_RECORD_ENTRY: &str = "";

const PROJECT_HEAD: &str = r#"{
  "schema_version": 1,
  "watermark": "SYNTHETIC DEMO DATA",
  "issuers": [
    { "id": "quillfield-consulting-engineers", "name": "Quillfield Consulting Engineers",
      "role": "Fictional consulting engineer.", "fictional": true },
    { "id": "harborstone-escrow-trust", "name": "Harborstone Escrow Trust",
      "role": "Fictional escrow trustee.", "fictional": true },
    { "id": "lanternbay-developments", "name": "Lanternbay Developments",
      "role": "Fictional developer.", "fictional": true }
  ],
  "documents": [
    "#;

const PROJECT_TAIL: &str = r#"{ "doc_id": "SR-01", "title": "Site progress report SR-01",
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

/// The fixture's `project.json`, with `record` as its first `documents` entry.
pub fn project_json(record: &str) -> String {
    format!("{PROJECT_HEAD}{record}{PROJECT_TAIL}")
}

/// The fixture's `project.json` with a record body that cites documents as
/// evidence, one citation per citing field the record format has.
///
/// The five fields are the five a reading consumer resolves against
/// `documents`, and each takes its own `doc_id` so a test can tell which field
/// a failure came from.
pub fn project_json_citing(
    record: &str,
    milestone_evidence: &str,
    contractor_note: &str,
    certificate_document: &str,
    schedule_document: &str,
    latest_attestation: &str,
) -> String {
    let declarations = project_json(record);
    let open = declarations
        .trim_end()
        .strip_suffix('}')
        .expect("the fixture record ends with its closing brace");
    format!(
        "{open},
  \"milestones\": [
    {{ \"id\": \"MS-01\", \"evidence\": [\"{milestone_evidence}\"],
      \"contractor_note\": \"{contractor_note}\" }}
  ],
  \"certificates\": [
    {{ \"id\": \"CERT-01\", \"document\": \"{certificate_document}\" }}
  ],
  \"payment_schedules\": [
    {{ \"unit\": \"1204\", \"document\": \"{schedule_document}\" }}
  ],
  \"escrow\": {{ \"latest_attestation\": \"{latest_attestation}\" }}
}}
"
    )
}

/// A temporary directory removed when the test ends.
pub struct Scratch {
    root: PathBuf,
}

impl Scratch {
    pub fn new(label: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ask-aunova-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("create the scratch directory");
        Scratch { root }
    }

    pub fn dataset_dir(&self) -> PathBuf {
        self.root.join("data").join("palm-vista")
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    /// A second key directory, for keys that are deliberately not on record.
    pub fn outsider_keys_dir(&self) -> PathBuf {
        self.root.join("outsider-keys")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Build the fixture dataset, unsigned, covering its own `project.json`.
pub fn fixture(label: &str) -> Scratch {
    fixture_with_record(label, PROJECT_RECORD_ENTRY)
}

/// Build the fixture dataset, unsigned, with `record` spliced in as the first
/// `documents` entry.
pub fn fixture_with_record(label: &str, record: &str) -> Scratch {
    let scratch = Scratch::new(label);
    let dataset_dir = scratch.dataset_dir();
    let docs_dir = dataset_dir.join("docs");
    std::fs::create_dir_all(&docs_dir).expect("create the fixture docs directory");
    std::fs::write(dataset_dir.join("project.json"), project_json(record))
        .expect("write project.json");
    write_document(
        &docs_dir,
        "site-progress-report-SR-01.md",
        "structure topped out",
    );
    write_document(
        &docs_dir,
        "site-progress-report-SR-02.md",
        "facade installation",
    );
    write_document(
        &docs_dir,
        "escrow-attestation-ESC-01.md",
        "escrow balance held",
    );
    scratch
}

/// Build the fixture dataset and sign it, generating the three issuer
/// keypairs.
pub fn signed_fixture(label: &str) -> Scratch {
    let scratch = fixture(label);
    sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), true).expect("sign the fixture");
    scratch
}

fn write_document(docs_dir: &Path, name: &str, body: &str) {
    let text = format!("# SYNTHETIC DEMO DATA\n\nFictional document: {body}.\n");
    std::fs::write(docs_dir.join(name), text).expect("write a fixture document");
}

/// Read the fixture's manifest.
pub fn load_manifest(scratch: &Scratch) -> Manifest {
    Manifest::load(&scratch.dataset_dir()).expect("load the fixture manifest")
}

/// Write a modified manifest back over the fixture's.
pub fn save_manifest(scratch: &Scratch, manifest: &Manifest) {
    manifest
        .write(&scratch.dataset_dir())
        .expect("write the fixture manifest");
}

/// Flip one byte of a fixture document, leaving its length unchanged.
pub fn flip_one_byte(scratch: &Scratch, document_path: &str) {
    let path = Project::document_path(&scratch.dataset_dir(), document_path);
    let mut bytes = std::fs::read(&path).expect("read a fixture document");
    assert!(
        !bytes.is_empty(),
        "fixture document {document_path} is empty"
    );
    bytes[0] ^= 0x20;
    std::fs::write(&path, bytes).expect("write a tampered fixture document");
}
