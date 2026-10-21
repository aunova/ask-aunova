//! The canonical signing payload has to be injective: two different field
//! tuples must never produce the same bytes, or a signature could be moved
//! between documents.

use dataset_manifest::canonical::{SIGNING_CONTEXT, SIGNING_PAYLOAD_DESCRIPTION, signing_payload};

#[test]
fn the_payload_is_exactly_the_documented_length_prefixed_form() {
    let payload = signing_payload("palm-vista", "SR-05", "quillfield", "docs/a.md", "ab12");
    let expected = format!(
        "{}:{SIGNING_CONTEXT}10:palm-vista5:SR-0510:quillfield9:docs/a.md4:ab12",
        SIGNING_CONTEXT.len()
    );
    assert_eq!(
        String::from_utf8(payload).expect("the payload is UTF-8 for ASCII fields"),
        expected
    );
}

#[test]
fn moving_a_character_between_adjacent_fields_changes_the_payload() {
    // Concatenation without length prefixes would make these two identical,
    // which is precisely the transplant the prefixes prevent.
    let left = signing_payload("palm-vista", "SR", "quillfield", "0docs/a.md", "ab12");
    let right = signing_payload("palm-vista", "SR0", "quillfield", "docs/a.md", "ab12");
    assert_ne!(left, right);
}

#[test]
fn a_newline_inside_a_field_cannot_fake_a_field_boundary() {
    // A newline-separated encoding would read these as the same five fields.
    let left = signing_payload("palm-vista", "SR-05\nquillfield", "x", "docs/a.md", "ab12");
    let right = signing_payload("palm-vista", "SR-05", "quillfield\nx", "docs/a.md", "ab12");
    assert_ne!(left, right);
}

#[test]
fn the_description_recorded_in_the_manifest_names_every_signed_field() {
    for field in [
        SIGNING_CONTEXT,
        "dataset",
        "doc_id",
        "issuer",
        "path",
        "sha256",
    ] {
        assert!(
            SIGNING_PAYLOAD_DESCRIPTION.contains(field),
            "signing_payload description does not mention {field:?}, so a third \
             party could not reproduce the payload from the manifest alone"
        );
    }
}
