//! Reading bytes off disk, with the two properties the rest of the crate needs:
//! every whole-file read is capped, and the document hash is streamed so a large
//! document cannot be turned into a large allocation.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

use crate::error::{DatasetError, display_path};

/// Cap on any single JSON control file (`project.json`, `manifest.json`,
/// `issuers.json`). Generous for hand-written synthetic data, small enough that
/// a corrupt or hostile file cannot be read into memory unbounded.
pub const JSON_READ_LIMIT: usize = 8 * 1024 * 1024;

/// Buffer used to stream a document through the hasher.
const HASH_CHUNK: usize = 64 * 1024;

/// Read a whole text file, refusing anything over `limit`.
///
/// The read asks for `limit + 1` bytes so an oversized file is *detected*
/// rather than silently truncated into a hash of the first `limit` bytes.
pub fn read_capped(path: &Path, what: &'static str, limit: usize) -> Result<String, DatasetError> {
    let file = File::open(path).map_err(|source| DatasetError::Read {
        what,
        path: display_path(path),
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| DatasetError::Read {
            what,
            path: display_path(path),
            source,
        })?;
    if bytes.len() > limit {
        return Err(DatasetError::TooLarge {
            what,
            path: display_path(path),
            limit,
        });
    }
    String::from_utf8(bytes).map_err(|_| DatasetError::Invalid {
        what,
        path: display_path(path),
        problems: vec!["the file is not valid UTF-8".to_string()],
    })
}

/// sha256 of bytes already in memory, lowercase hex.
///
/// This is what a caller uses when it must hash *the buffer it is going to
/// read from* rather than the path it came from: hashing a path and then
/// opening it again checks one set of bytes and uses another.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// sha256 of a file's exact bytes, lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String, DatasetError> {
    let mut file = File::open(path).map_err(|source| DatasetError::Read {
        what: "a dataset document",
        path: display_path(path),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; HASH_CHUNK];
    loop {
        let read = file.read(&mut chunk).map_err(|source| DatasetError::Read {
            what: "a dataset document",
            path: display_path(path),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Every regular file under `dir`, as paths relative to `base` with `/`
/// separators, sorted.
///
/// Symlinks are reported as files rather than followed, so a symlinked
/// directory under `docs/` shows up as one undeclared entry instead of being
/// walked into (which also makes a symlink loop impossible here).
pub fn list_files_relative(base: &Path, dir: &Path) -> Result<Vec<String>, DatasetError> {
    let mut found = Vec::new();
    walk(base, dir, &mut found)?;
    found.sort();
    Ok(found)
}

fn walk(base: &Path, dir: &Path, found: &mut Vec<String>) -> Result<(), DatasetError> {
    let entries = std::fs::read_dir(dir).map_err(|source| DatasetError::Read {
        what: "the dataset document directory",
        path: display_path(dir),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| DatasetError::Read {
            what: "an entry of the dataset document directory",
            path: display_path(dir),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| DatasetError::Read {
            what: "the type of a dataset document directory entry",
            path: display_path(&path),
            source,
        })?;
        if file_type.is_dir() {
            walk(base, &path, found)?;
        } else {
            found.push(relative_slash_path(base, &path)?);
        }
    }
    Ok(())
}

fn relative_slash_path(base: &Path, path: &Path) -> Result<String, DatasetError> {
    let relative = path.strip_prefix(base).map_err(|_| DatasetError::Invalid {
        what: "a dataset document",
        path: display_path(path),
        problems: vec!["the file is not inside the dataset directory".to_string()],
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => {
                return Err(DatasetError::Invalid {
                    what: "a dataset document",
                    path: display_path(path),
                    problems: vec![
                        "the path leaves the dataset directory instead of naming a file inside it"
                            .to_string(),
                    ],
                });
            }
        }
    }
    Ok(parts.join("/"))
}
