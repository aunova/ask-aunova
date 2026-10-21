//! Issuer keys: load, generate, sign, verify.
//!
//! A private key is a 32-byte Ed25519 seed written as 64 lowercase-hex
//! characters and a newline, in a file only its owner can read. It never leaves
//! `keys/private/`, which is gitignored; the seed and its hex text are wiped
//! from memory as soon as the signing key exists.

use std::io::Write;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use crate::error::{DatasetError, display_path};

/// Subdirectory of the key directory holding private keys. Gitignored.
pub const PRIVATE_SUBDIR: &str = "private";

/// File extension for a private key.
pub const PRIVATE_KEY_EXTENSION: &str = "ed25519";

/// Bytes of an Ed25519 secret seed.
const SEED_BYTES: usize = 32;
/// Bytes of an Ed25519 public key.
const PUBLIC_KEY_BYTES: usize = 32;
/// Bytes of an Ed25519 signature.
const SIGNATURE_BYTES: usize = 64;

/// Where an issuer's private key lives, relative to `keys_dir`.
pub fn private_key_path(keys_dir: &Path, issuer_id: &str) -> PathBuf {
    keys_dir
        .join(PRIVATE_SUBDIR)
        .join(format!("{issuer_id}.{PRIVATE_KEY_EXTENSION}"))
}

/// Load an issuer's signing key.
pub fn load_signing_key(keys_dir: &Path, issuer_id: &str) -> Result<SigningKey, DatasetError> {
    let path = private_key_path(keys_dir, issuer_id);
    if !path.exists() {
        return Err(DatasetError::MissingPrivateKey {
            issuer: issuer_id.to_string(),
            path: display_path(&path),
        });
    }
    // Zeroizing so the hex text of the seed does not outlive this function.
    let text =
        Zeroizing::new(
            std::fs::read_to_string(&path).map_err(|source| DatasetError::Read {
                what: "an issuer private key",
                path: display_path(&path),
                source,
            })?,
        );
    let trimmed = text.trim();
    let seed: Zeroizing<[u8; SEED_BYTES]> = Zeroizing::new(
        decode_hex(
            trimmed,
            "64 lowercase-hex characters (a 32-byte Ed25519 seed)",
        )
        .map_err(|error| match error {
            DatasetError::MalformedHex { expected, found } => DatasetError::MalformedPrivateKey {
                issuer: issuer_id.to_string(),
                path: display_path(&path),
                expected,
                found,
            },
            other => other,
        })?,
    );
    Ok(SigningKey::from_bytes(&seed))
}

/// Create an issuer's private key, failing rather than overwriting an existing
/// one. A silently regenerated key would invalidate an already committed
/// manifest without saying so.
pub fn generate_signing_key(keys_dir: &Path, issuer_id: &str) -> Result<SigningKey, DatasetError> {
    let path = private_key_path(keys_dir, issuer_id);
    let directory = keys_dir.join(PRIVATE_SUBDIR);
    std::fs::create_dir_all(&directory).map_err(|source| DatasetError::CreateDir {
        what: "issuer private keys",
        path: display_path(&directory),
        source,
    })?;

    let mut seed: Zeroizing<[u8; SEED_BYTES]> = Zeroizing::new([0u8; SEED_BYTES]);
    getrandom::fill(seed.as_mut()).map_err(|source| DatasetError::Entropy {
        bytes: SEED_BYTES,
        source,
    })?;

    let mut file = create_owner_only(&path)?;
    let mut encoded = Zeroizing::new(hex::encode(seed.as_ref()));
    encoded.push('\n');
    file.write_all(encoded.as_bytes())
        .map_err(|source| DatasetError::Write {
            what: "a new issuer private key",
            path: display_path(&path),
            source,
        })?;
    file.sync_all().map_err(|source| DatasetError::Write {
        what: "a new issuer private key",
        path: display_path(&path),
        source,
    })?;

    Ok(SigningKey::from_bytes(&seed))
}

/// The issuer's public key, lowercase hex.
pub fn public_key_hex(signing_key: &SigningKey) -> String {
    hex::encode(signing_key.verifying_key().to_bytes())
}

/// Sign a canonical payload, returning a lowercase-hex signature.
pub fn sign_payload(
    signing_key: &SigningKey,
    doc_id: &str,
    payload: &[u8],
) -> Result<String, DatasetError> {
    let signature = signing_key
        .try_sign(payload)
        .map_err(|error| DatasetError::Sign {
            doc_id: doc_id.to_string(),
            detail: error.to_string(),
        })?;
    Ok(hex::encode(signature.to_bytes()))
}

/// Check a signature over a canonical payload.
///
/// `Err` means the manifest entry is malformed (bad hex, wrong length, or a
/// public key that is not a valid curve point). `Ok(false)` means the entry is
/// well formed and the signature does not verify.
///
/// Uses `verify_strict`, not the `Verifier` trait's `verify`. The crate's own
/// documentation for `VerifyingKey` states that a low-order ("weak") public
/// key can produce a signature valid for almost every message, that plain
/// `verify` accepts such keys, and that `verify_strict` denies them along with
/// a low-order `R`. We take the stricter path, and we do not rely on it alone:
/// a signature is only accepted if its public key is also the one recorded for
/// that issuer in the committed `keys/public/issuers.json`, which is a check we
/// own and test ourselves.
pub fn verify_payload(
    public_key_hex: &str,
    payload: &[u8],
    signature_hex: &str,
) -> Result<bool, DatasetError> {
    let key_bytes: [u8; PUBLIC_KEY_BYTES] = decode_hex(
        public_key_hex,
        "64 lowercase-hex characters (a 32-byte Ed25519 public key)",
    )?;
    let signature_bytes: [u8; SIGNATURE_BYTES] = decode_hex(
        signature_hex,
        "128 lowercase-hex characters (a 64-byte Ed25519 signature)",
    )?;
    let verifying_key =
        VerifyingKey::from_bytes(&key_bytes).map_err(|_| DatasetError::MalformedHex {
            expected: "an Ed25519 public key on the curve".to_string(),
            found: "32 bytes that do not decompress to a curve point".to_string(),
        })?;
    let signature = Signature::from_bytes(&signature_bytes);
    Ok(verifying_key.verify_strict(payload, &signature).is_ok())
}

#[cfg(unix)]
fn create_owner_only(path: &Path) -> Result<std::fs::File, DatasetError> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| DatasetError::Write {
            what: "a new issuer private key",
            path: display_path(path),
            source,
        })
}

#[cfg(not(unix))]
fn create_owner_only(path: &Path) -> Result<std::fs::File, DatasetError> {
    Err(DatasetError::UnsupportedKeyPermissions {
        path: display_path(path),
    })
}

fn decode_hex<const N: usize>(value: &str, expected: &str) -> Result<[u8; N], DatasetError> {
    let uppercase = value.bytes().any(|byte| byte.is_ascii_uppercase());
    if value.len() != N * 2 || uppercase {
        return Err(DatasetError::MalformedHex {
            expected: expected.to_string(),
            found: format!(
                "{} characters{}",
                value.len(),
                if uppercase {
                    ", some of them uppercase"
                } else {
                    ""
                }
            ),
        });
    }
    let mut decoded = [0u8; N];
    hex::decode_to_slice(value, &mut decoded).map_err(|_| DatasetError::MalformedHex {
        expected: expected.to_string(),
        found: "characters outside 0-9a-f".to_string(),
    })?;
    Ok(decoded)
}
