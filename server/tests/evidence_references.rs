//! The citations the server resolves are the citations the verifier checks.
//!
//! `verify-dataset` refuses a record that cites a `doc_id` it does not
//! declare, and it finds those citations with its own reader
//! (`dataset_manifest::project`), because the signing tools cannot depend on
//! the server. Two readers of one format drift: a citing field added to
//! [`ProjectRecord`] and not to the verifier's reader is a citation that gate
//! would pass as sound, and the only thing left standing between it and a
//! caller would be the role filter in `answers.rs`.
//!
//! So both readers are run over the committed record and their citation sets
//! compared. This catches a field the record actually uses; a field added to
//! the model that the committed record never fills is beyond what any test
//! here can see, which is why the comparison is over the real dataset rather
//! than a fixture written to suit it.

use std::{collections::BTreeSet, fs, path::PathBuf};

use ask_aunova_server::record::ProjectRecord;
use dataset_manifest::project::Project;

/// The committed dataset's record, as text.
fn committed_record_text() -> String {
    fs::read_to_string(repository_root().join("data/palm-vista/project.json"))
        .expect("read the committed project record")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the server package sits inside the repository")
        .to_path_buf()
}

/// Every `doc_id` the server resolves as evidence, from the five fields that
/// carry one.
fn cited_by_the_server(record: &ProjectRecord) -> BTreeSet<String> {
    let mut cited = BTreeSet::new();
    for milestone in &record.milestones {
        cited.extend(milestone.evidence.iter().cloned());
        cited.extend(milestone.contractor_note.iter().cloned());
    }
    for certificate in &record.certificates {
        cited.insert(certificate.document.clone());
    }
    for schedule in &record.payment_schedules {
        cited.insert(schedule.document.clone());
    }
    cited.insert(record.escrow.latest_attestation.clone());
    cited
}

#[test]
fn both_readers_of_the_record_find_the_same_citations() {
    let text = committed_record_text();
    let record: ProjectRecord = serde_json::from_str(&text).expect("the committed record parses");
    let project = Project::from_text(&text, "data/palm-vista/project.json")
        .expect("the committed record validates");

    let server = cited_by_the_server(&record);
    let verifier: BTreeSet<String> = project
        .evidence_references
        .iter()
        .map(|reference| reference.doc_id.clone())
        .collect();

    assert!(
        !server.is_empty(),
        "the committed record cites no evidence at all, so this test proves nothing",
    );
    assert_eq!(
        server, verifier,
        "the two readers of the record disagree about what it cites, so a citation the \
         server resolves is one `verify-dataset` does not check (or the other way about)",
    );
}

/// The committed dataset declares every document it cites.
///
/// Checked by hand once on Day 1 over one query; this is the same question
/// asked by the gate, on the dataset the demo ships.
#[test]
fn the_committed_record_cites_nothing_it_does_not_declare() {
    let text = committed_record_text();
    let project = Project::from_text(&text, "data/palm-vista/project.json")
        .expect("the committed record validates");
    let declared: BTreeSet<&str> = project
        .documents
        .iter()
        .map(|document| document.doc_id.as_str())
        .collect();

    let dangling: Vec<&str> = project
        .evidence_references
        .iter()
        .filter(|reference| !declared.contains(reference.doc_id.as_str()))
        .map(|reference| reference.cited_by.as_str())
        .collect();
    assert!(
        dangling.is_empty(),
        "the committed record cites documents it does not declare: {dangling:?}",
    );
}
