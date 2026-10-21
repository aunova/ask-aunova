//! The `whats_changed` cursor: what the *client* carries, so the server holds
//! nothing.
//!
//! # Why there is no ledger behind this
//!
//! The obvious design — the server remembers what it has told each caller —
//! records a delivery the moment the answer is composed, and an answer is not
//! a delivery. A client that times out, drops the connection or is killed
//! between the two has been told nothing, and the server has written down that
//! it was told. Measured, that was not a rare race: fifteen of fifteen
//! abandoned calls lost both changes, permanently, because nothing in HTTP or
//! in MCP tells a handler that its response never arrived.
//!
//! So nothing is recorded. The answer carries a cursor, and the next call
//! carries it back. A client can only send back a cursor it actually received,
//! which makes receipt the thing that moves the mark — and an abandoned call
//! loses exactly nothing, because the cursor that would have advanced it was
//! in the response that never arrived.
//!
//! # Why ids and not a date
//!
//! A change this server could not state — its evidence did not verify in that
//! call — must still be offered afterwards. A single date cannot express that:
//! report a newer change, and an older held-back one is behind the watermark
//! for good. The cursor therefore carries the change ids that were delivered,
//! one at a time, and a change is offered until its id is in a cursor the
//! caller sends back.
//!
//! # What "opaque" means here, exactly
//!
//! Opaque to the client, not secret from it. The body is the hex of the change
//! ids this caller was already told about, so it hides nothing from the person
//! holding it and is not a credential — it is marked `v1.` so a later encoding
//! can be told apart from this one, and a client that tries to compose one by
//! hand is doing something this server does not promise to keep working.
//! A cursor is never trusted on its own. Every id in one is matched against
//! the changes the caller's *role* may hear in the call that reads it, and an
//! id survives only when this caller may read a document that evidences the
//! change it names. A cursor from another caller therefore shrinks to the
//! changes this caller could have been told about anyway, and never grows:
//! there is no id a caller can put in one and have confirmed back.

use std::collections::BTreeSet;

/// The marker every cursor this build issues starts with.
pub const CURSOR_PREFIX: &str = "v1.";
/// Longest cursor accepted, in bytes. The record cannot produce one a tenth
/// this size; the cap is here so an untrusted argument is bounded before it is
/// decoded, not because a real cursor approaches it.
pub const MAX_CURSOR_BYTES: usize = 8192;
/// Most change ids a cursor may carry.
pub const MAX_CURSOR_ENTRIES: usize = 256;

/// Why a cursor could not be read.
///
/// Each one names what was wrong, because the alternative — treating anything
/// unreadable as "from the beginning" — answers a question the caller did not
/// ask and hides the fault that caused it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    /// Longer than [`MAX_CURSOR_BYTES`].
    #[error("a cursor is at most {MAX_CURSOR_BYTES} bytes; this one is {length}")]
    TooLong {
        /// Length of what arrived.
        length: usize,
    },
    /// Not a cursor this build issued.
    #[error(
        "a cursor must be one this server issued, which starts with `{CURSOR_PREFIX}`; send \
         the `next_cursor` from your previous answer, or omit the argument to start from the \
         baseline"
    )]
    UnknownVersion,
    /// The body is not lowercase hex.
    #[error("a cursor's body is lowercase hexadecimal; byte {offset} of this one is not")]
    NotHexadecimal {
        /// Which byte of the body broke the rule.
        offset: usize,
    },
    /// An odd number of hex digits cannot be whole bytes.
    #[error("a cursor's body has an odd number of hexadecimal digits, so it is not whole bytes")]
    OddLength,
    /// The decoded bytes are not text.
    #[error("a cursor decodes to UTF-8 text; this one does not")]
    NotText,
    /// More ids than [`MAX_CURSOR_ENTRIES`].
    #[error("a cursor carries at most {MAX_CURSOR_ENTRIES} change ids; this one carries {count}")]
    TooManyEntries {
        /// How many arrived.
        count: usize,
    },
    /// An empty id names no change.
    #[error("a cursor's change ids are all non-empty; entry {index} of this one is empty")]
    EmptyEntry {
        /// Which entry broke the rule.
        index: usize,
    },
}

/// Encode the change ids a caller has now been told about.
///
/// The set is ordered, so the same delivery always produces the same cursor
/// and two identical calls cannot be told apart by their cursors.
pub fn encode(delivered: &BTreeSet<String>) -> String {
    let joined = delivered
        .iter()
        .map(String::as_str)
        .collect::<Vec<&str>>()
        .join("\n");
    format!("{CURSOR_PREFIX}{}", hex::encode(joined))
}

/// Read a cursor back into the change ids it carries.
pub fn decode(cursor: &str) -> Result<BTreeSet<String>, CursorError> {
    if cursor.len() > MAX_CURSOR_BYTES {
        return Err(CursorError::TooLong {
            length: cursor.len(),
        });
    }
    let body = cursor
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(CursorError::UnknownVersion)?;
    if body.is_empty() {
        // The cursor a caller gets when nothing was delivered: valid, and it
        // excludes nothing.
        return Ok(BTreeSet::new());
    }
    // Uppercase is refused before the body is decoded: a cursor has exactly
    // one spelling, so two strings standing for the same set cannot both be
    // in circulation. `hex` accepts either case, so this check is ours — the
    // same one `decode_hex` makes in the signing crate.
    if let Some(offset) = body.bytes().position(|digit| digit.is_ascii_uppercase()) {
        return Err(CursorError::NotHexadecimal { offset });
    }
    let bytes = hex::decode(body).map_err(|error| match error {
        hex::FromHexError::InvalidHexCharacter { index, .. } => {
            CursorError::NotHexadecimal { offset: index }
        }
        // `OddLength` is the only other error `hex::decode` returns.
        // `InvalidStringLength` belongs to decoding into a fixed-size
        // container, which this is not, so it cannot arrive here; it is
        // mapped rather than unwrapped because a panic in a request handler
        // takes the server down, and a caller controls this argument.
        hex::FromHexError::OddLength | hex::FromHexError::InvalidStringLength => {
            CursorError::OddLength
        }
    })?;

    let text = String::from_utf8(bytes).map_err(|_not_text| CursorError::NotText)?;
    let entries: Vec<&str> = text.split('\n').collect();
    if entries.len() > MAX_CURSOR_ENTRIES {
        return Err(CursorError::TooManyEntries {
            count: entries.len(),
        });
    }
    for (index, entry) in entries.iter().enumerate() {
        if entry.is_empty() {
            return Err(CursorError::EmptyEntry { index });
        }
    }
    Ok(entries.into_iter().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(entries: &[&str]) -> BTreeSet<String> {
        entries.iter().map(|entry| (*entry).to_owned()).collect()
    }

    #[test]
    fn a_cursor_round_trips_the_ids_it_was_given() {
        let delivered = ids(&[
            "certificate:CERT-ESCROW-2026-209:2026-10-05",
            "certificate:CERT-STRUCT-2026-114:2026-09-04",
        ]);
        let cursor = encode(&delivered);
        assert!(cursor.starts_with(CURSOR_PREFIX), "{cursor}");
        assert_eq!(decode(&cursor), Ok(delivered));
    }

    #[test]
    fn an_empty_delivery_is_a_cursor_that_excludes_nothing() {
        let cursor = encode(&BTreeSet::new());
        assert_eq!(cursor, CURSOR_PREFIX);
        assert_eq!(decode(&cursor), Ok(BTreeSet::new()));
    }

    #[test]
    fn the_same_delivery_always_encodes_to_the_same_cursor() {
        let one = encode(&ids(&["b:2", "a:1"]));
        let two = encode(&ids(&["a:1", "b:2"]));
        assert_eq!(
            one, two,
            "a set has one encoding, whatever order it arrived"
        );
    }

    #[test]
    fn a_malformed_cursor_is_refused_and_never_read_as_the_beginning() {
        assert_eq!(decode("milestone:MS-07"), Err(CursorError::UnknownVersion));
        assert_eq!(decode(""), Err(CursorError::UnknownVersion));
        assert_eq!(decode("v2.616263"), Err(CursorError::UnknownVersion));
        assert_eq!(decode("v1.abc"), Err(CursorError::OddLength));
        assert_eq!(
            decode("v1.6g"),
            Err(CursorError::NotHexadecimal { offset: 1 }),
        );
        assert_eq!(
            decode("v1.6A"),
            Err(CursorError::NotHexadecimal { offset: 1 }),
            "a cursor has one spelling, so uppercase hex is not it",
        );
        assert_eq!(decode("v1.ff"), Err(CursorError::NotText));
        assert_eq!(
            decode("v1.610a0a62"),
            Err(CursorError::EmptyEntry { index: 1 }),
        );
    }

    #[test]
    fn an_oversized_cursor_is_refused_before_it_is_decoded() {
        let long = format!("{CURSOR_PREFIX}{}", "61".repeat(MAX_CURSOR_BYTES));
        assert_eq!(
            decode(&long),
            Err(CursorError::TooLong { length: long.len() }),
        );

        let many = encode(&(0..=MAX_CURSOR_ENTRIES).map(|n| format!("k:{n}")).collect());
        assert_eq!(
            decode(&many),
            Err(CursorError::TooManyEntries {
                count: MAX_CURSOR_ENTRIES + 1
            }),
        );
    }

    #[test]
    fn every_refusal_says_what_was_wrong() {
        for error in [
            CursorError::TooLong { length: 9000 },
            CursorError::UnknownVersion,
            CursorError::NotHexadecimal { offset: 3 },
            CursorError::OddLength,
            CursorError::NotText,
            CursorError::TooManyEntries { count: 300 },
            CursorError::EmptyEntry { index: 2 },
        ] {
            let message = error.to_string();
            assert!(
                message.len() > 40 && message.contains("cursor"),
                "a cursor refusal must explain itself: {message}",
            );
        }
    }
}
