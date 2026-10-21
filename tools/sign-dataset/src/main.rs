#![forbid(unsafe_code)]

//! Hash, sign and record every document of a synthetic Ask Aunova dataset.
//!
//! Writes `<dataset>/manifest.json` and the committed issuer public key list
//! at `<keys>/public/issuers.json`. Private keys stay in `<keys>/private/`,
//! which is gitignored.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use dataset_manifest::sign::sign_dataset;

#[derive(Parser)]
#[command(
    name = "sign-dataset",
    version,
    about = "Sign every document of a synthetic Ask Aunova dataset.",
    long_about = "Reads <dataset-dir>/project.json, hashes every document it declares with \
                  sha256, signs each one with its issuer's Ed25519 key, and writes \
                  <dataset-dir>/manifest.json together with the committed issuer public key \
                  list. Refuses to sign, writing nothing, if a file sits under docs/ without \
                  being declared, if two entries share a doc_id or a path, or if the dataset \
                  does not declare project.json itself."
)]
struct Args {
    /// Dataset directory, relative to the current directory.
    #[arg(default_value = "data/palm-vista")]
    dataset_dir: PathBuf,

    /// Key directory holding private/<issuer>.ed25519 and public/issuers.json.
    #[arg(long, default_value = "keys")]
    keys_dir: PathBuf,

    /// Create a keypair for any issuer that does not have one yet, instead of
    /// failing. Never regenerates a key that already exists.
    #[arg(long)]
    generate_missing_keys: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match sign_dataset(
        &args.dataset_dir,
        &args.keys_dir,
        args.generate_missing_keys,
    ) {
        Ok(outcome) => {
            for issuer in &outcome.generated_keys {
                println!("generated a new keypair for issuer \"{issuer}\"");
            }
            println!(
                "signed {} documents from {} issuers in dataset \"{}\"",
                outcome.documents, outcome.issuers, outcome.dataset
            );
            println!("wrote {}", outcome.manifest_path.display());
            println!("wrote {}", outcome.issuers_path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sign-dataset: {error}");
            ExitCode::FAILURE
        }
    }
}
