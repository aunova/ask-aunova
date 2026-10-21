#![forbid(unsafe_code)]

//! Re-check a signed Ask Aunova dataset: every hash, every signature, and
//! every public key against the committed issuer list — plus the rules that
//! are about the manifest as a whole rather than one entry: one entry per
//! `doc_id` and per path, one entry per issuer in the committed key list, and
//! `project.json` itself covered — plus the record's own cross-references: a
//! `doc_id` cited as evidence but never declared is signed by nothing.
//!
//! Exits 0 on a clean dataset, non-zero with every failure named otherwise.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use dataset_manifest::verify::verify_dataset;

#[derive(Parser)]
#[command(
    name = "verify-dataset",
    version,
    about = "Verify a signed synthetic Ask Aunova dataset.",
    long_about = "For every document the dataset declares: recomputes its sha256 and compares \
                  it to the manifest, verifies the manifest's Ed25519 signature over the \
                  canonical payload, and cross-checks the manifest's issuer public key against \
                  the committed list in <keys>/public/issuers.json. Also refuses a manifest \
                  that repeats a doc_id or a path, a key list that repeats an issuer, a \
                  manifest that does not sign project.json itself, and a record that cites a \
                  doc_id as evidence without declaring it in \"documents\", which leaves every \
                  fact resting on it signed by nothing. Reports every failure found, then \
                  exits non-zero."
)]
struct Args {
    /// Dataset directory, relative to the current directory.
    #[arg(default_value = "data/palm-vista")]
    dataset_dir: PathBuf,

    /// Key directory holding public/issuers.json.
    #[arg(long, default_value = "keys")]
    keys_dir: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match verify_dataset(&args.dataset_dir, &args.keys_dir) {
        Ok(outcome) if outcome.is_clean() => {
            println!(
                "verified {} documents from {} issuers in dataset \"{}\"",
                outcome.verified, outcome.issuers, outcome.dataset
            );
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            let failed = outcome.failures.len();
            eprintln!(
                "verify-dataset: {} of {} declared documents verified in dataset \"{}\"; \
                 {failed} {} failed:",
                outcome.verified,
                outcome.declared(),
                outcome.dataset,
                if failed == 1 { "check" } else { "checks" }
            );
            for failure in &outcome.failures {
                eprintln!("  {failure}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("verify-dataset: {error}");
            ExitCode::FAILURE
        }
    }
}
