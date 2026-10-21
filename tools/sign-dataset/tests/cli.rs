//! End-to-end runs of the real `sign-dataset` binary over a throwaway dataset
//! built inside the process temp directory (`TMPDIR`, pointed at the repo's
//! `.qa/tmp`). Never the real `data/palm-vista`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use dataset_manifest::issuers::IssuerKeys;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

const ISSUER: &str = "quillfield-consulting-engineers";
const DEVELOPER: &str = "lanternbay-developments";

/// A dataset that covers its own record: `project.json` appears in its own
/// `documents`, so editing it is caught like any other document.
const PROJECT_JSON: &str = r#"{
  "watermark": "SYNTHETIC DEMO DATA",
  "issuers": [
    { "id": "quillfield-consulting-engineers", "name": "Quillfield Consulting Engineers" },
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
      "issuer": "quillfield-consulting-engineers", "visibility": "public" }
  ]
}
"#;

/// The same dataset with the record left out — review finding F1.
const PROJECT_JSON_WITHOUT_RECORD: &str = r#"{
  "watermark": "SYNTHETIC DEMO DATA",
  "issuers": [
    { "id": "quillfield-consulting-engineers", "name": "Quillfield Consulting Engineers" },
    { "id": "lanternbay-developments", "name": "Lanternbay Developments" }
  ],
  "documents": [
    { "doc_id": "SR-01", "title": "Site progress report SR-01",
      "path": "docs/site-progress-report-SR-01.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    { "doc_id": "SR-02", "title": "Site progress report SR-02",
      "path": "docs/site-progress-report-SR-02.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" }
  ]
}
"#;

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "ask-aunova-sign-cli-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        let docs = root.join("data").join("palm-vista").join("docs");
        std::fs::create_dir_all(&docs).expect("create the scratch dataset");
        std::fs::write(
            root.join("data").join("palm-vista").join("project.json"),
            PROJECT_JSON,
        )
        .expect("write project.json");
        for name in [
            "site-progress-report-SR-01.md",
            "site-progress-report-SR-02.md",
        ] {
            std::fs::write(
                docs.join(name),
                format!("# SYNTHETIC DEMO DATA\n\nFictional document {name}.\n"),
            )
            .expect("write a fixture document");
        }
        Scratch { root }
    }

    fn dataset_dir(&self) -> PathBuf {
        self.root.join("data").join("palm-vista")
    }

    fn keys_dir(&self) -> PathBuf {
        self.root.join("keys")
    }

    /// Run the real binary with the scratch root as its working directory, so
    /// the default `data/palm-vista` and `keys` arguments are exercised.
    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sign-dataset"))
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("run sign-dataset")
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
fn signing_with_generated_keys_writes_a_manifest_and_the_public_key_list() {
    let scratch = Scratch::new("generate");
    let output = scratch.run(&["--generate-missing-keys"]);
    assert!(
        output.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout(&output),
        stderr(&output)
    );
    let text = stdout(&output);
    assert!(
        text.contains(&format!("generated a new keypair for issuer \"{ISSUER}\"")),
        "{text}"
    );
    assert!(
        text.contains("signed 3 documents from 2 issuers in dataset \"palm-vista\""),
        "{text}"
    );

    assert!(scratch.dataset_dir().join("manifest.json").exists());
    let issuers = IssuerKeys::load(&scratch.keys_dir()).expect("load issuers.json");
    assert_eq!(issuers.issuers.len(), 2);
    assert!(issuers.public_key(ISSUER).is_some());
    assert!(issuers.public_key(DEVELOPER).is_some());
}

#[cfg(unix)]
#[test]
fn a_generated_private_key_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("permissions");
    assert!(scratch.run(&["--generate-missing-keys"]).status.success());
    let key = scratch
        .keys_dir()
        .join("private")
        .join(format!("{ISSUER}.ed25519"));
    let mode = std::fs::metadata(&key)
        .expect("stat the private key")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "private key mode is {mode:o}, expected 600");
}

#[test]
fn re_signing_without_the_flag_reuses_the_existing_keys_and_changes_nothing() {
    let scratch = Scratch::new("rerun");
    assert!(scratch.run(&["--generate-missing-keys"]).status.success());
    let manifest_path = scratch.dataset_dir().join("manifest.json");
    let first = std::fs::read(&manifest_path).expect("read the first manifest");

    let output = scratch.run(&[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !stdout(&output).contains("generated a new keypair"),
        "a second run regenerated a key: {}",
        stdout(&output)
    );
    let second = std::fs::read(&manifest_path).expect("read the second manifest");
    assert_eq!(first, second, "re-signing changed manifest.json");
}

#[test]
fn signing_without_a_key_names_the_issuer_and_the_flag() {
    let scratch = Scratch::new("no-key");
    let output = scratch.run(&[]);
    assert!(!output.status.success(), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains(ISSUER), "{message}");
    assert!(message.contains("--generate-missing-keys"), "{message}");
    assert!(
        !scratch.dataset_dir().join("manifest.json").exists(),
        "a manifest was written despite the missing key"
    );
}

#[test]
fn signing_refuses_a_file_that_is_not_declared() {
    let scratch = Scratch::new("undeclared");
    std::fs::write(
        scratch.dataset_dir().join("docs").join("smuggled.md"),
        "# SYNTHETIC DEMO DATA\n\nNot declared.\n",
    )
    .expect("write a smuggled document");

    let output = scratch.run(&["--generate-missing-keys"]);
    assert!(!output.status.success(), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("docs/smuggled.md"), "{message}");
    assert!(
        !scratch.dataset_dir().join("manifest.json").exists(),
        "a manifest was written despite the undeclared document"
    );
}

#[test]
fn an_explicit_dataset_directory_is_honoured() {
    let scratch = Scratch::new("explicit");
    let output = scratch.run(&[
        "data/palm-vista",
        "--keys-dir",
        "keys",
        "--generate-missing-keys",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(Path::new(&scratch.dataset_dir().join("manifest.json")).exists());
}

/// Review finding F1: covering `project.json` is a rule of the tool, not a
/// convention of one dataset. A dataset that leaves its own record out is
/// refused, and the refusal writes nothing — no manifest, no key.
#[test]
fn signing_refuses_a_dataset_that_does_not_cover_its_own_record() {
    let scratch = Scratch::new("no-record");
    std::fs::write(
        scratch.dataset_dir().join("project.json"),
        PROJECT_JSON_WITHOUT_RECORD,
    )
    .expect("write a project.json that leaves out its own record");

    let output = scratch.run(&["--generate-missing-keys"]);
    assert!(!output.status.success(), "{}", stdout(&output));
    let message = stderr(&output);
    assert!(message.contains("project.json"), "{message}");
    assert!(
        !scratch.dataset_dir().join("manifest.json").exists(),
        "a manifest was written despite the uncovered record"
    );
    assert!(
        !scratch.keys_dir().exists(),
        "a keypair was generated despite the refusal"
    );
}
