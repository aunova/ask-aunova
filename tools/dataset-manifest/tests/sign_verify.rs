//! The adversarial matrix: every way a signed dataset can be wrong, and the
//! proof that `verify_dataset` names it.

mod common;

use std::fs;

use dataset_manifest::canonical::signing_payload;
use dataset_manifest::files::sha256_bytes;
use dataset_manifest::keys::{
    generate_signing_key, load_signing_key, public_key_hex, sign_payload,
};
use dataset_manifest::manifest::Manifest;
use dataset_manifest::project::Project;
use dataset_manifest::sign::sign_dataset;
use dataset_manifest::verify::{
    CheckFailure, DatasetFault, DocumentFault, FailureScope, VerifyOutcome, verify_dataset,
};

use common::{
    ISSUER_A, ISSUER_B, ISSUER_C, NO_RECORD_ENTRY, PROJECT_RECORD_ENTRY, RENAMED_RECORD_ENTRY,
    Scratch, fixture, fixture_with_record, flip_one_byte, load_manifest, project_json,
    project_json_citing, save_manifest, signed_fixture,
};

fn verify(scratch: &Scratch) -> VerifyOutcome {
    verify_dataset(&scratch.dataset_dir(), &scratch.keys_dir()).expect("verification runs")
}

fn only_failure(outcome: &VerifyOutcome) -> &CheckFailure {
    assert_eq!(
        outcome.failures.len(),
        1,
        "expected exactly one failure, got: {}",
        describe(outcome)
    );
    &outcome.failures[0]
}

fn describe(outcome: &VerifyOutcome) -> String {
    outcome
        .failures
        .iter()
        .map(|failure| failure.to_string())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Every manifest-level problem the run reported, in order.
fn header_problems(outcome: &VerifyOutcome) -> Vec<&str> {
    outcome
        .failures
        .iter()
        .filter_map(|failure| match failure {
            CheckFailure::Header { problem } => Some(problem.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_freshly_signed_dataset_verifies_clean() {
    let scratch = signed_fixture("clean");
    let outcome = verify(&scratch);
    assert!(outcome.is_clean(), "{}", describe(&outcome));
    assert_eq!(outcome.verified, 4);
    assert_eq!(outcome.declared(), 4);
    assert_eq!(outcome.issuers, 3);
}

/// The bytes that were hashed are the bytes the caller gets back.
///
/// A consumer that needs more out of the record than these tools do parses
/// `project_json`. If that string were a second read of the same path, a
/// writer between the two reads would have its content verified and the
/// caller's content served — so what is asserted here is that the string
/// hashes to exactly what the manifest signed.
#[test]
fn the_record_handed_back_is_the_record_that_was_hashed() {
    let scratch = signed_fixture("single-read");
    let outcome = verify(&scratch);
    assert!(outcome.is_clean(), "{}", describe(&outcome));

    let entry = outcome
        .manifest
        .document("PROJECT-RECORD")
        .expect("the manifest signs the project record");
    assert_eq!(
        sha256_bytes(outcome.project_json.as_bytes()),
        entry.sha256,
        "the text returned is not the text whose hash was checked",
    );
    assert_eq!(
        outcome.project_json,
        fs::read_to_string(scratch.dataset_dir().join("project.json"))
            .expect("read the record back"),
        "and it is the file's own content, not a re-serialisation of it",
    );
}

/// One rule for "this fault belongs to the dataset, not to a document",
/// shared by `verify-dataset` and by any other consumer.
///
/// A duplicate entry leaves no document to mark: both rows claim the same
/// `doc_id`, and every hash and signature still checks out. A consumer that
/// degrades per document would find nothing to degrade and answer anyway.
#[test]
fn a_duplicate_entry_is_a_fault_of_the_dataset_and_of_no_document() {
    let scratch = signed_fixture("dataset-scope");
    let mut manifest = load_manifest(&scratch);
    let copy = manifest.documents[index_of(&manifest, "SR-01")].clone();
    manifest.documents.push(copy);
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert!(!outcome.is_clean(), "a duplicate entry must not verify");
    assert!(
        outcome.failures.iter().all(|failure| matches!(
            failure.scope(),
            FailureScope::Dataset {
                fault: DatasetFault::ManifestSet
            }
        )),
        "{}",
        describe(&outcome),
    );
    let faults = outcome.dataset_faults();
    assert_eq!(faults.len(), outcome.failures.len());
    assert!(
        faults
            .iter()
            .any(|(_kind, detail)| detail.contains("SR-01")),
        "{faults:?}",
    );
    assert!(
        faults
            .iter()
            .all(|(kind, _detail)| !kind.label().contains("SR-01")),
        "the kind of a fault is what a caller who may not read the dataset is told, so it \
         must name nothing in it: {faults:?}",
    );

    // Every hash and signature in this dataset is still sound, which is the
    // point: a per-document view sees a clean dataset.
    assert_eq!(
        outcome.verified,
        outcome.declared(),
        "no document is at fault here: {}",
        describe(&outcome),
    );
}

#[test]
fn signing_writes_a_public_key_for_every_issuer() {
    let scratch = signed_fixture("public-keys");
    let issuers = dataset_manifest::issuers::IssuerKeys::load(&scratch.keys_dir())
        .expect("load issuers.json");
    assert_eq!(issuers.issuers.len(), 3);
    for issuer_id in [ISSUER_A, ISSUER_B, ISSUER_C] {
        let key = issuers
            .public_key(issuer_id)
            .unwrap_or_else(|| panic!("{issuer_id} has no public key on record"));
        assert_eq!(
            key.len(),
            64,
            "{issuer_id} public key is not 32 bytes of hex"
        );
        assert!(
            key.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }
}

#[test]
fn re_signing_an_unchanged_dataset_is_byte_identical() {
    let scratch = signed_fixture("determinism");
    let manifest_path = Manifest::path_in(&scratch.dataset_dir());
    let first = std::fs::read(&manifest_path).expect("read the first manifest");
    sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), false).expect("re-sign");
    let second = std::fs::read(&manifest_path).expect("read the second manifest");
    assert_eq!(first, second, "re-signing changed manifest.json");
}

#[test]
fn one_flipped_byte_is_a_hash_failure_naming_that_document() {
    let scratch = signed_fixture("flipped-byte");
    flip_one_byte(&scratch, "docs/site-progress-report-SR-02.md");
    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 3);
    match only_failure(&outcome) {
        CheckFailure::Hash {
            doc_id,
            path,
            recorded,
            actual,
        } => {
            assert_eq!(doc_id.as_str(), "SR-02");
            assert_eq!(path.as_str(), "docs/site-progress-report-SR-02.md");
            assert_ne!(recorded, actual);
        }
        other => panic!("expected a hash failure, got: {other}"),
    }
}

#[test]
fn a_signature_made_by_a_different_issuer_does_not_verify() {
    let scratch = signed_fixture("wrong-issuer");
    let wrong_key = load_signing_key(&scratch.keys_dir(), ISSUER_B).expect("load issuer B's key");
    let mut manifest = load_manifest(&scratch);
    let dataset = manifest.dataset.clone();
    {
        let entry = manifest
            .documents
            .iter_mut()
            .find(|entry| entry.doc_id == "SR-01")
            .expect("SR-01 is in the manifest");
        assert_eq!(entry.issuer.as_str(), ISSUER_A);
        let payload = signing_payload(
            &dataset,
            &entry.doc_id,
            &entry.issuer,
            &entry.path,
            &entry.sha256,
        );
        // Issuer B signs it, but the entry keeps issuer A's id and public key,
        // so the committed-key cross-check is satisfied and only the signature
        // can catch this.
        entry.signature = sign_payload(&wrong_key, &entry.doc_id, &payload).expect("sign");
    }
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    match only_failure(&outcome) {
        CheckFailure::Signature { doc_id, path, .. } => {
            assert_eq!(doc_id.as_str(), "SR-01");
            assert_eq!(path.as_str(), "docs/site-progress-report-SR-01.md");
        }
        other => panic!("expected a signature failure, got: {other}"),
    }
}

/// The single most important check in the program. Every hash matches and
/// every signature is cryptographically valid — and the dataset is still
/// rejected, because the keys are not the ones on record.
#[test]
fn an_attacker_who_re_signs_the_whole_manifest_is_caught_by_the_committed_key_list() {
    let scratch = signed_fixture("unknown-key");
    let attacker = generate_signing_key(&scratch.outsider_keys_dir(), "outsider")
        .expect("generate an outsider key");
    let mut manifest = load_manifest(&scratch);
    let dataset = manifest.dataset.clone();
    for entry in &mut manifest.documents {
        let payload = signing_payload(
            &dataset,
            &entry.doc_id,
            &entry.issuer,
            &entry.path,
            &entry.sha256,
        );
        entry.issuer_public_key = public_key_hex(&attacker);
        entry.signature = sign_payload(&attacker, &entry.doc_id, &payload).expect("sign");
    }
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 0, "{}", describe(&outcome));
    assert_eq!(outcome.failures.len(), 4, "{}", describe(&outcome));
    for failure in &outcome.failures {
        match failure {
            CheckFailure::UnknownKey {
                doc_id,
                path,
                manifest_key,
                ..
            } => {
                assert!(!doc_id.is_empty());
                assert!(path.starts_with("docs/") || path == "project.json");
                assert_eq!(manifest_key.as_str(), public_key_hex(&attacker));
            }
            other => panic!("expected an unknown-key failure, got: {other}"),
        }
    }
}

#[test]
fn swapping_two_signatures_from_the_same_issuer_is_rejected() {
    let scratch = signed_fixture("signature-swap");
    let mut manifest = load_manifest(&scratch);
    let first = index_of(&manifest, "SR-01");
    let second = index_of(&manifest, "SR-02");
    assert_eq!(
        manifest.documents[first].issuer, manifest.documents[second].issuer,
        "the swap only proves the binding if both entries share an issuer"
    );
    let moved = manifest.documents[first].signature.clone();
    manifest.documents[first].signature = manifest.documents[second].signature.clone();
    manifest.documents[second].signature = moved;
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 2, "{}", describe(&outcome));
    assert_eq!(outcome.failures.len(), 2, "{}", describe(&outcome));
    let named: Vec<&str> = outcome
        .failures
        .iter()
        .map(|failure| match failure {
            CheckFailure::Signature { doc_id, .. } => doc_id.as_str(),
            other => panic!("expected a signature failure, got: {other}"),
        })
        .collect();
    assert!(
        named.contains(&"SR-01") && named.contains(&"SR-02"),
        "{named:?}"
    );
}

#[test]
fn transplanting_one_entry_onto_another_document_is_rejected() {
    let scratch = signed_fixture("transplant");
    let mut manifest = load_manifest(&scratch);
    let source = index_of(&manifest, "SR-01");
    let target = index_of(&manifest, "SR-02");
    // SR-02 keeps its own doc_id and path but claims SR-01's hash and
    // signature — the shape an attacker would use to pass off one document's
    // proof as another's.
    manifest.documents[target].sha256 = manifest.documents[source].sha256.clone();
    manifest.documents[target].signature = manifest.documents[source].signature.clone();
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 3, "{}", describe(&outcome));
    let mut saw_hash = false;
    let mut saw_signature = false;
    for failure in &outcome.failures {
        match failure {
            CheckFailure::Hash { doc_id, path, .. } => {
                assert_eq!(doc_id.as_str(), "SR-02");
                assert_eq!(path.as_str(), "docs/site-progress-report-SR-02.md");
                saw_hash = true;
            }
            CheckFailure::Signature { doc_id, .. } => {
                assert_eq!(doc_id.as_str(), "SR-02");
                saw_signature = true;
            }
            other => panic!("unexpected failure: {other}"),
        }
    }
    assert!(
        saw_hash,
        "the swapped hash was not caught: {}",
        describe(&outcome)
    );
    assert!(
        saw_signature,
        "the transplanted signature was not caught: {}",
        describe(&outcome)
    );
}

#[test]
fn a_document_missing_from_the_manifest_is_reported() {
    let scratch = signed_fixture("missing-entry");
    let mut manifest = load_manifest(&scratch);
    manifest.documents.retain(|entry| entry.doc_id != "ESC-01");
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 3);
    // Dropping the entry leaves the file on disk uncovered, so the document is
    // reported twice: once as declared-but-unsigned, once as an unsigned file.
    assert_eq!(outcome.failures.len(), 2, "{}", describe(&outcome));
    let mut saw_missing = false;
    let mut saw_unsigned = false;
    for failure in &outcome.failures {
        match failure {
            CheckFailure::MissingFromManifest { doc_id, path } => {
                assert_eq!(doc_id.as_str(), "ESC-01");
                assert_eq!(path.as_str(), "docs/escrow-attestation-ESC-01.md");
                saw_missing = true;
            }
            CheckFailure::UnsignedFile { path } => {
                assert_eq!(path.as_str(), "docs/escrow-attestation-ESC-01.md");
                saw_unsigned = true;
            }
            other => panic!("unexpected failure: {other}"),
        }
    }
    assert!(saw_missing && saw_unsigned, "{}", describe(&outcome));
}

#[test]
fn a_manifest_entry_the_project_does_not_declare_is_reported() {
    let scratch = signed_fixture("ghost-entry");
    let mut manifest = load_manifest(&scratch);
    let mut ghost = manifest.documents[index_of(&manifest, "SR-01")].clone();
    ghost.doc_id = "GHOST-01".to_string();
    // A path of its own: an entry that reused SR-01's path would be a
    // duplicate as well, and this test is about the undeclared entry alone.
    ghost.path = "docs/site-progress-report-GHOST-01.md".to_string();
    manifest.documents.push(ghost);
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 4, "{}", describe(&outcome));
    match only_failure(&outcome) {
        CheckFailure::NotInProject { doc_id, path } => {
            assert_eq!(doc_id.as_str(), "GHOST-01");
            assert_eq!(path.as_str(), "docs/site-progress-report-GHOST-01.md");
        }
        other => panic!("expected a not-in-project failure, got: {other}"),
    }
}

#[test]
fn a_manifest_entry_whose_file_is_gone_is_reported() {
    let scratch = signed_fixture("missing-file");
    let path = Project::document_path(&scratch.dataset_dir(), "docs/escrow-attestation-ESC-01.md");
    std::fs::remove_file(&path).expect("remove a fixture document");

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 3);
    match only_failure(&outcome) {
        CheckFailure::MissingFile { doc_id, path } => {
            assert_eq!(doc_id.as_str(), "ESC-01");
            assert_eq!(path.as_str(), "docs/escrow-attestation-ESC-01.md");
        }
        other => panic!("expected a missing-file failure, got: {other}"),
    }
}

#[test]
fn a_file_added_under_docs_after_signing_is_reported_as_unsigned() {
    let scratch = signed_fixture("unsigned-file");
    std::fs::write(
        scratch.dataset_dir().join("docs").join("smuggled.md"),
        "# SYNTHETIC DEMO DATA\n\nNot declared, not signed.\n",
    )
    .expect("write a smuggled document");

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 4);
    match only_failure(&outcome) {
        CheckFailure::UnsignedFile { path } => assert_eq!(path.as_str(), "docs/smuggled.md"),
        other => panic!("expected an unsigned-file failure, got: {other}"),
    }
}

#[test]
fn a_manifest_that_names_another_algorithm_is_not_checked_silently() {
    let scratch = signed_fixture("algorithm");
    let mut manifest = load_manifest(&scratch);
    manifest.algorithm.signature = "ed448".to_string();
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    match only_failure(&outcome) {
        CheckFailure::Header { problem } => {
            assert!(problem.contains("ed448"), "{problem}");
            assert!(problem.contains("ed25519"), "{problem}");
        }
        other => panic!("expected a header failure, got: {other}"),
    }
}

#[test]
fn signing_refuses_a_dataset_with_an_undeclared_document() {
    let scratch = fixture("undeclared");
    std::fs::write(
        scratch.dataset_dir().join("docs").join("smuggled.md"),
        "# SYNTHETIC DEMO DATA\n\nSmuggled in before signing.\n",
    )
    .expect("write a smuggled document");

    let error = sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), true)
        .expect_err("signing must refuse an undeclared document");
    let message = error.to_string();
    assert!(message.contains("docs/smuggled.md"), "{message}");
    assert!(
        !Manifest::path_in(&scratch.dataset_dir()).exists(),
        "a manifest was written despite the undeclared document"
    );
}

#[test]
fn signing_refuses_a_missing_private_key_unless_asked_to_generate_one() {
    let scratch = fixture("missing-key");
    let error = sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), false)
        .expect_err("signing must refuse without a key");
    let message = error.to_string();
    assert!(message.contains(ISSUER_A), "{message}");
    assert!(message.contains("--generate-missing-keys"), "{message}");
}

#[test]
fn project_json_problems_name_the_entry_and_the_field() {
    let scratch = fixture("bad-project");
    std::fs::write(
        scratch.dataset_dir().join("project.json"),
        r#"{
  "watermark": "SYNTHETIC DEMO DATA",
  "issuers": [
    { "id": "quillfield-consulting-engineers", "name": "Quillfield Consulting Engineers" }
  ],
  "documents": [
    { "doc_id": "SR-01", "title": "t", "issuer": "quillfield-consulting-engineers",
      "visibility": "public" },
    { "doc_id": "SR-02", "title": "t", "path": "docs/site-progress-report-SR-02.md",
      "issuer": "nobody-at-all", "visibility": "public" },
    { "doc_id": "SR-03", "title": "t", "path": "../../escape.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" }
  ]
}
"#,
    )
    .expect("write a broken project.json");

    let error = Project::load(&scratch.dataset_dir()).expect_err("validation must fail");
    let message = error.to_string();
    assert!(
        message.contains("documents[0] (doc_id \"SR-01\") is missing the required field \"path\""),
        "{message}"
    );
    assert!(message.contains("nobody-at-all"), "{message}");
    assert!(message.contains("\"..\""), "{message}");
}

// --- Review finding F1: the record the dataset answers from must stay signed.

#[test]
fn signing_refuses_a_dataset_that_does_not_cover_its_own_record() {
    let scratch = fixture_with_record("sign-no-record", NO_RECORD_ENTRY);
    let error = sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), true)
        .expect_err("signing must refuse a dataset that does not cover project.json");
    let message = error.to_string();
    assert!(message.contains("project.json"), "{message}");
    assert!(
        !Manifest::path_in(&scratch.dataset_dir()).exists(),
        "a manifest was written despite the uncovered record"
    );
    assert!(
        !scratch.keys_dir().exists(),
        "a keypair was generated despite the refusal; the refusal must write nothing at all"
    );
}

/// Review finding F1, case D1, in its worst form: the attacker edits
/// `project.json` and deletes its entry from *both* files, so no per-document
/// check has anything to catch. Only a rule about the manifest as a whole can
/// see it.
#[test]
fn a_manifest_that_covers_no_record_at_all_is_rejected() {
    let scratch = signed_fixture("verify-no-record");
    std::fs::write(
        scratch.dataset_dir().join("project.json"),
        project_json(NO_RECORD_ENTRY),
    )
    .expect("rewrite project.json without its own record");
    let mut manifest = load_manifest(&scratch);
    manifest
        .documents
        .retain(|entry| entry.path != "project.json");
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 3, "{}", describe(&outcome));
    match only_failure(&outcome) {
        CheckFailure::Header { problem } => {
            assert!(problem.contains("project.json"), "{problem}");
        }
        other => panic!("expected a header failure, got: {other}"),
    }
}

#[test]
fn a_record_declared_but_not_signed_is_reported_as_missing_and_as_uncovered() {
    let scratch = signed_fixture("record-unsigned");
    let mut manifest = load_manifest(&scratch);
    manifest
        .documents
        .retain(|entry| entry.path != "project.json");
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 3, "{}", describe(&outcome));
    assert_eq!(outcome.failures.len(), 2, "{}", describe(&outcome));
    let mut saw_missing = false;
    let mut saw_uncovered = false;
    for failure in &outcome.failures {
        match failure {
            CheckFailure::MissingFromManifest { doc_id, path } => {
                assert_eq!(doc_id.as_str(), "PROJECT-RECORD");
                assert_eq!(path.as_str(), "project.json");
                saw_missing = true;
            }
            CheckFailure::Header { problem } => {
                assert!(problem.contains("project.json"), "{problem}");
                saw_uncovered = true;
            }
            other => panic!("unexpected failure: {other}"),
        }
    }
    assert!(saw_missing && saw_uncovered, "{}", describe(&outcome));
}

/// What is signed is the path and the content hash, so the id a dataset gives
/// its own record is its business. A check keyed on one fixed `doc_id` would
/// fail this honest dataset and catch nothing an attacker would do.
#[test]
fn a_record_signed_under_another_doc_id_still_counts_as_covered() {
    let scratch = fixture_with_record("record-renamed", RENAMED_RECORD_ENTRY);
    sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), true).expect("sign the fixture");
    let outcome = verify(&scratch);
    assert!(outcome.is_clean(), "{}", describe(&outcome));
    assert_eq!(outcome.verified, 4);
}

/// The reason all of this matters: with the record covered, editing a fact in
/// it is a hash failure naming the record, instead of silence.
#[test]
fn editing_the_covered_record_is_a_hash_failure_naming_it() {
    let scratch = signed_fixture("record-edited");
    let path = scratch.dataset_dir().join("project.json");
    let before = std::fs::read_to_string(&path).expect("read the fixture record");
    let after = before.replace("\"schema_version\": 1", "\"schema_version\": 2");
    assert_ne!(before, after, "the fixture record did not contain the edit");
    std::fs::write(&path, after).expect("write the edited record");

    let outcome = verify(&scratch);
    assert_eq!(outcome.verified, 3, "{}", describe(&outcome));
    match only_failure(&outcome) {
        CheckFailure::Hash {
            doc_id,
            path,
            recorded,
            actual,
        } => {
            assert_eq!(doc_id.as_str(), "PROJECT-RECORD");
            assert_eq!(path.as_str(), "project.json");
            assert_ne!(recorded, actual);
        }
        other => panic!("expected a hash failure, got: {other}"),
    }
}

// --- Review finding F3: one key, one entry.

/// The reproduction from the review, verbatim: a second entry for one
/// `doc_id`, carrying a hash of 64 zeroes and a signature of 128 ones. The
/// verifier used to take the first entry and call the dataset clean, while a
/// consumer building a map from the same file would keep this one.
#[test]
fn a_second_entry_for_one_doc_id_is_rejected() {
    let scratch = signed_fixture("duplicate-doc-id");
    let mut manifest = load_manifest(&scratch);
    let mut bogus = manifest.documents[index_of(&manifest, "SR-01")].clone();
    bogus.sha256 = "0".repeat(64);
    bogus.signature = "1".repeat(128);
    manifest.documents.push(bogus);
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    // The copied row duplicates both keys, so both are named.
    let problems = header_problems(&outcome);
    assert_eq!(outcome.failures.len(), 2, "{}", describe(&outcome));
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("doc_id \"SR-01\"") && problem.contains("2 times")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|problem| {
            problem.contains("path \"docs/site-progress-report-SR-01.md\"")
                && problem.contains("2 times")
        }),
        "{problems:?}"
    );
}

#[test]
fn every_duplicate_is_reported_not_only_the_first() {
    let scratch = signed_fixture("duplicate-many");
    let mut manifest = load_manifest(&scratch);
    for doc_id in ["SR-01", "ESC-01"] {
        let mut bogus = manifest.documents[index_of(&manifest, doc_id)].clone();
        bogus.sha256 = "0".repeat(64);
        bogus.signature = "1".repeat(128);
        manifest.documents.push(bogus);
    }
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    let problems = header_problems(&outcome);
    assert_eq!(problems.len(), 4, "{}", describe(&outcome));
    for named in [
        "doc_id \"SR-01\"",
        "doc_id \"ESC-01\"",
        "path \"docs/site-progress-report-SR-01.md\"",
        "path \"docs/escrow-attestation-ESC-01.md\"",
    ] {
        assert!(
            problems.iter().any(|problem| problem.contains(named)),
            "{named} is not named in {problems:?}"
        );
    }
}

/// The same defect under the other key: a second entry pointing at one file,
/// under a `doc_id` of its own.
#[test]
fn a_second_entry_pointing_at_one_file_is_rejected() {
    let scratch = signed_fixture("duplicate-path");
    let mut manifest = load_manifest(&scratch);
    let mut bogus = manifest.documents[index_of(&manifest, "SR-01")].clone();
    bogus.doc_id = "SR-01-COPY".to_string();
    bogus.sha256 = "0".repeat(64);
    manifest.documents.push(bogus);
    save_manifest(&scratch, &manifest);

    let outcome = verify(&scratch);
    // Twice: one file with two entries, and an entry project.json never
    // declared.
    assert_eq!(outcome.failures.len(), 2, "{}", describe(&outcome));
    assert!(
        header_problems(&outcome).iter().any(|problem| {
            problem.contains("path \"docs/site-progress-report-SR-01.md\"")
                && problem.contains("2 times")
        }),
        "{}",
        describe(&outcome)
    );
    assert!(
        outcome.failures.iter().any(|failure| matches!(
            failure,
            CheckFailure::NotInProject { doc_id, .. } if doc_id == "SR-01-COPY"
        )),
        "{}",
        describe(&outcome)
    );
}

/// The trust anchor cannot have two answers: a second entry for one issuer in
/// the committed key list is a second answer to which key that issuer uses.
#[test]
fn a_second_entry_for_one_issuer_in_the_committed_key_list_is_rejected() {
    let scratch = signed_fixture("duplicate-issuer");
    let mut keys = dataset_manifest::issuers::IssuerKeys::load(&scratch.keys_dir())
        .expect("load issuers.json");
    let mut twin = keys
        .issuers
        .iter()
        .find(|issuer| issuer.id == ISSUER_B)
        .expect("issuer B is on record")
        .clone();
    twin.public_key = "0".repeat(64);
    keys.issuers.push(twin);
    keys.write(&scratch.keys_dir())
        .expect("write the tampered key list");

    let outcome = verify(&scratch);
    match only_failure(&outcome) {
        CheckFailure::Header { problem } => {
            assert!(problem.contains("issuers.json"), "{problem}");
            assert!(problem.contains(ISSUER_B), "{problem}");
            assert!(problem.contains("2 times"), "{problem}");
        }
        other => panic!("expected a header failure, got: {other}"),
    }
}

/// Signing runs the same rule, so it cannot write a manifest its own verifier
/// would reject. Two entries for one file pass `project.json` validation —
/// their `doc_id`s differ — and are stopped here.
#[test]
fn signing_refuses_two_entries_for_one_file() {
    let duplicate = format!(
        "{PROJECT_RECORD_ENTRY}{}",
        r#"{ "doc_id": "SR-01-COPY", "title": "A second entry for one file",
      "path": "docs/site-progress-report-SR-01.md",
      "issuer": "quillfield-consulting-engineers", "visibility": "public" },
    "#
    );
    let scratch = fixture_with_record("sign-duplicate-path", &duplicate);
    let error = sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), true)
        .expect_err("signing must refuse two entries for one file");
    let message = error.to_string();
    assert!(
        message.contains("docs/site-progress-report-SR-01.md"),
        "{message}"
    );
    assert!(message.contains("2 times"), "{message}");
    assert!(
        !Manifest::path_in(&scratch.dataset_dir()).exists(),
        "a manifest was written despite the duplicate"
    );
    assert!(
        !scratch.keys_dir().exists(),
        "a keypair was generated despite the refusal"
    );
}

/// Sign a fixture whose record cites documents, and verify it.
fn signed_citing(label: &str, cited: [&str; 5]) -> Scratch {
    let scratch = fixture(label);
    fs::write(
        scratch.dataset_dir().join("project.json"),
        project_json_citing(
            PROJECT_RECORD_ENTRY,
            cited[0],
            cited[1],
            cited[2],
            cited[3],
            cited[4],
        ),
    )
    .expect("write a record that cites its evidence");
    sign_dataset(&scratch.dataset_dir(), &scratch.keys_dir(), true).expect("sign the fixture");
    scratch
}

/// A `doc_id` cited as evidence but never declared is a failure, from every
/// field that can cite one.
///
/// This is the gap r4's follow-on names: every signature in such a dataset is
/// sound, every file is signed, and the manifest is a set a reader can act on
/// — so until this check existed, `verify-dataset` exited 0 on a record
/// citing a document that nothing in the dataset signs. The five fields are
/// the five a reading consumer resolves, and each is given its own id so a
/// field that stopped being checked is named rather than merely missed.
#[test]
fn a_record_citing_a_document_it_does_not_declare_does_not_verify() {
    let cited = [
        "MISSING-EVIDENCE",
        "MISSING-NOTE",
        "MISSING-CERTIFICATE",
        "MISSING-SCHEDULE",
        "MISSING-ATTESTATION",
    ];
    let scratch = signed_citing("dangling-evidence", cited);
    let outcome = verify(&scratch);

    assert!(
        !outcome.is_clean(),
        "a record citing documents nothing signs must not verify",
    );
    for doc_id in cited {
        let found = outcome.failures.iter().any(|failure| {
            matches!(failure, CheckFailure::DanglingEvidence { doc_id: cited, .. } if cited == doc_id)
        });
        assert!(
            found,
            "{doc_id} was cited and not reported: {}",
            describe(&outcome)
        );
    }
    assert_eq!(
        outcome.failures.len(),
        cited.len(),
        "{}",
        describe(&outcome),
    );

    // Each failure names the field that cites it, which is what an author
    // needs to find it in a record of any size.
    let text = describe(&outcome);
    for field in [
        "\"evidence\"",
        "\"contractor_note\"",
        "\"document\"",
        "\"latest_attestation\"",
    ] {
        assert!(text.contains(field), "{field} is not named: {text}");
    }

    // One dangling id leaves one id to mark, so it is a per-document fault:
    // a reader that degrades per document drops that id's facts and keeps
    // answering from the documents that did verify.
    assert!(
        outcome.failures.iter().all(|failure| matches!(
            failure.scope(),
            FailureScope::Document {
                fault: DocumentFault::Absent,
                ..
            }
        )),
        "{text}",
    );
    assert!(
        outcome.dataset_faults().is_empty(),
        "a dangling citation is not a fault of the dataset as a set: {:?}",
        outcome.dataset_faults(),
    );

    // Every signature in the dataset is still sound, which is the point: no
    // other check in this verifier looks at the record's body.
    assert_eq!(
        outcome.verified,
        outcome.declared(),
        "no declared document is at fault here: {text}",
    );
}

/// The control: the same five fields citing documents the record does declare
/// verify clean, so the check is a cross-reference and not a ban on citing.
#[test]
fn a_record_citing_only_declared_documents_verifies_clean() {
    let scratch = signed_citing(
        "declared-evidence",
        ["SR-01", "SR-02", "ESC-01", "SR-01", "ESC-01"],
    );
    let outcome = verify(&scratch);
    assert!(outcome.is_clean(), "{}", describe(&outcome));
    assert_eq!(outcome.verified, outcome.declared());
}

fn index_of(manifest: &Manifest, doc_id: &str) -> usize {
    manifest
        .documents
        .iter()
        .position(|entry| entry.doc_id == doc_id)
        .unwrap_or_else(|| panic!("{doc_id} is not in the manifest"))
}
