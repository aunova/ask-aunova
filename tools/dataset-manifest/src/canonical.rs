//! The exact byte string an issuer signs for one document.
//!
//! WHY LENGTH-PREFIXED AND NOT NEWLINE-SEPARATED. A separator-based encoding is
//! only unambiguous if the fields are *known* never to contain the separator.
//! Three of these six fields (`dataset`, `doc_id`, `path`) are copied out of a
//! JSON file, and JSON strings can carry a newline, a colon or anything else.
//! With a separator, two different field tuples could then produce the same
//! payload, which is exactly the transplant we are trying to prevent. Prefixing
//! each field with its byte length makes the encoding injective for arbitrary
//! field contents, so it needs no assumption about the data at all. It is also
//! trivial for a third party to reproduce: it is the netstring shape, minus the
//! trailing comma.

/// Domain separator. Signed first, so a signature made for this dataset format
/// can never be replayed as a signature over some other Ask Aunova payload.
pub const SIGNING_CONTEXT: &str = "ask-aunova/dataset-entry/v1";

/// Human-readable description of [`signing_payload`], recorded verbatim in
/// `manifest.json` so a third party can reproduce a signature without reading
/// this source file.
pub const SIGNING_PAYLOAD_DESCRIPTION: &str = concat!(
    "ed25519 over the concatenation of six length-prefixed UTF-8 fields, in this order: ",
    "context \"ask-aunova/dataset-entry/v1\", dataset, doc_id, issuer, path, sha256. ",
    "Each field is encoded as its length in bytes written in ASCII decimal, then a colon, ",
    "then the field's UTF-8 bytes. There is no separator between fields and none at the end. ",
    "Example for a one-field payload \"ab\": 2:ab"
);

/// Build the canonical payload for one manifest entry.
///
/// Binding `doc_id`, `issuer`, `path` and `sha256` together is the point: a
/// signature cannot be transplanted onto another document, re-attributed to
/// another issuer, moved to another path, or kept while the hash is swapped.
pub fn signing_payload(
    dataset: &str,
    doc_id: &str,
    issuer: &str,
    path: &str,
    sha256_hex: &str,
) -> Vec<u8> {
    let fields = [SIGNING_CONTEXT, dataset, doc_id, issuer, path, sha256_hex];
    // length digits + ':' is at most 20 bytes for any field we can hold in memory.
    let capacity: usize = fields.iter().map(|field| field.len() + 20).sum();
    let mut payload = Vec::with_capacity(capacity);
    for field in fields {
        payload.extend_from_slice(field.len().to_string().as_bytes());
        payload.push(b':');
        payload.extend_from_slice(field.as_bytes());
    }
    payload
}
