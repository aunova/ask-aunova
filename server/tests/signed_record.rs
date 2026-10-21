//! The record that answers questions is the record whose signature was
//! checked.
//!
//! Verification hashes bytes; answers are composed from a parsed record. If
//! those two are different reads of the same path, a writer between them turns
//! "its signature checked out in this call" into a false sentence. This test
//! puts a writer there on purpose.

mod common;

use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use common::Fixture;

/// How many verification runs race the writer.
///
/// The window is one filesystem read wide, so a single pass would be a coin
/// toss. Six hundred makes an open window certain to be caught and a closed
/// one cheap to prove: the whole test is a few seconds of small-file I/O.
const RUNS: usize = 600;

/// The forged figure. One byte of the signed record, which is the edit the
/// demo makes on camera.
const SIGNED_PERCENT: &str = "\"percent_complete\": 48";
const FORGED_PERCENT: &str = "\"percent_complete\": 58";

#[test]
fn a_record_is_never_answered_from_bytes_that_were_not_the_bytes_verified() {
    let fixture = Fixture::new("record-race");
    let record_path = fixture.dataset_dir().join("project.json");
    let signed = fs::read_to_string(&record_path).expect("read the signed record");
    assert_eq!(
        signed.matches(SIGNED_PERCENT).count(),
        1,
        "the forgery must be a single edit for this test to mean anything",
    );
    let forged = signed.replacen(SIGNED_PERCENT, FORGED_PERCENT, 1);
    assert_eq!(
        signed.len(),
        forged.len(),
        "the forgery must be the same length, so the race is on content and not on size",
    );

    let config = fixture.config();
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        let path = record_path.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _forged = fs::write(&path, forged.as_bytes());
                let _signed = fs::write(&path, signed.as_bytes());
            }
        })
    };

    let mut answered = 0usize;
    let mut forgeries = Vec::new();
    for _run in 0..RUNS {
        let Ok(dataset) = config.load_and_verify() else {
            // A refusal is the correct outcome for bytes that do not verify,
            // and a half-written file refuses too.
            continue;
        };
        answered += 1;
        let facade = dataset
            .record
            .milestones
            .iter()
            .find(|milestone| milestone.id == "MS-07")
            .expect("the record carries MS-07");
        if facade.percent_complete != Some(48) {
            forgeries.push(facade.percent_complete);
        }
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().expect("the writer thread finished");

    assert!(
        forgeries.is_empty(),
        "{} of {answered} answered runs were composed from a record that was never verified \
         (MS-07 read as {:?}); the bytes that were hashed and the bytes that were parsed must \
         be the same read",
        forgeries.len(),
        forgeries,
    );
    assert!(
        answered > 0,
        "the race never let a single run through, so this test proved nothing",
    );
}
