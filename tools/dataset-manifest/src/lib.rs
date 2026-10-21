#![forbid(unsafe_code)]

//! Ask Aunova — the signed-dataset core shared by `sign-dataset` and
//! `verify-dataset`.
//!
//! A dataset is a directory holding `project.json`, a `docs/` tree of source
//! documents, and (once signed) a `manifest.json`. Each manifest entry binds a
//! document's id, issuer, path and sha256 under one Ed25519 signature, and the
//! issuer public keys that verify those signatures are committed separately in
//! `keys/public/issuers.json`.
//!
//! All data under `data/` is synthetic demonstration data from fictional
//! issuers, watermarked `SYNTHETIC DEMO DATA`.

pub mod canonical;
pub mod error;
pub mod files;
pub mod issuers;
pub mod keys;
pub mod manifest;
pub mod project;
pub mod sign;
/// Private: the set-level rules signing and verification must agree on.
/// Callers reach them through `Manifest::structural_problems` and
/// `IssuerKeys::duplicate_problems`.
mod structure;
pub mod verify;
