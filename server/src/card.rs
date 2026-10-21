//! The evidence card: one citation, rendered, served as a `ui://` resource.
//!
//! # Why a plain resource and not an MCP App
//!
//! The MCP Apps extension is negotiated through the generic `extensions` map in
//! client and server capabilities. rmcp 3.5.1 carries that map and nothing more
//! for this extension: `io.modelcontextprotocol/ui` appears in the crate only
//! as a doc-comment example of what an extension identifier looks like
//! (`.qa/vendor/rmcp-3.5.1/src/model/capabilities.rs:15`, `:26`, `:181`; the
//! sibling identifier `io.modelcontextprotocol/apps` likewise at `:227`) and in
//! that module's own tests (`:600`, `:615`, `:646`, `:656`). There is no
//! builder for it — `ServerCapabilitiesBuilder` offers `enable_tasks` and
//! nothing equivalent for apps or ui (`:429`, `:460`) — no `supports_ui` to
//! match `supports_tasks` (`:200`, `:248`), not one occurrence of a `ui://`
//! URI anywhere under `src/`, and no app mime type outside those doc examples
//! and tests (`mimeTypes` occurs only at `:28`, `:602`, `:615`, `:647`,
//! `:658`). What rmcp 3.5.1 does implement is two *other* extensions: Tasks
//! (`src/model/task.rs:13-14`, `src/task_manager.rs:1-2`) and OAuth client
//! credentials, SEP-1046 (`src/transport/auth.rs:951-952`, with
//! `configure_client_credentials` at `:3206`, `exchange_client_credentials`
//! at `:3270`, `authenticate_client_credentials` at `:4135` and a test
//! asserting the identifier at `:8589-8590`). Neither is MCP Apps, which is
//! the only one this module's decision rests on.
//!
//! So this server does **not** declare the extension: claiming support for a
//! negotiation it does not run would be exactly the kind of unearned claim this
//! project exists to avoid. It serves the card as an ordinary MCP resource
//! under a `ui://` URI, which any client can read through `resources/list` and
//! `resources/read`, and our own simulator renders.
//!
//! The mime type is [`EVIDENCE_CARD_MIME_TYPE`], the exact string rmcp's own
//! documentation gives for a UI resource
//! (`.qa/vendor/rmcp-3.5.1/src/model/capabilities.rs:28`): a client that later
//! learns the extension recognises the profile, and a client that does not
//! still sees `text/html`.
//!
//! The HTML is self-contained on purpose — no script, no stylesheet, no font
//! and no image fetched from anywhere — because an evidence card that loads
//! something from the network is a card whose contents are no longer the thing
//! that was signed.

use crate::verify::DocumentEvidence;

/// Every evidence card URI starts with this.
pub const EVIDENCE_CARD_URI_PREFIX: &str = "ui://ask-aunova/evidence/";

/// Mime type served for an evidence card. See the module documentation for why
/// this exact string.
pub const EVIDENCE_CARD_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Watermark carried by every rendered card, as everywhere else in this repo.
const WATERMARK: &str = "SYNTHETIC DEMO DATA";

/// The URI of one document's evidence card.
pub fn card_uri(doc_id: &str) -> String {
    format!("{EVIDENCE_CARD_URI_PREFIX}{doc_id}")
}

/// The document id inside an evidence-card URI, if it is one.
pub fn doc_id_from_uri(uri: &str) -> Option<&str> {
    uri.strip_prefix(EVIDENCE_CARD_URI_PREFIX)
        .filter(|doc_id| !doc_id.is_empty())
}

/// Render one citation as a self-contained HTML card.
///
/// `date` is the date the project record puts on this document, which only
/// certificates have; the card says so rather than borrowing another date.
pub fn render(evidence: &DocumentEvidence, date: Option<&str>, project_name: &str) -> String {
    let status = evidence.status.as_str();
    let (badge_background, badge_text) = match status {
        "verified" => ("#0f5132", "#d1e7dd"),
        _ => ("#842029", "#f8d7da"),
    };
    let detail = if evidence.detail.is_empty() {
        "Hash recomputed and Ed25519 signature re-checked against the issuer key on record, \
         during the call that produced this card."
            .to_owned()
    } else {
        evidence.detail.clone()
    };
    let hash = evidence
        .sha256
        .clone()
        .unwrap_or_else(|| "not recorded in the manifest".to_owned());

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Evidence · {doc_id}</title>
<style>
:root {{ color-scheme: light dark; }}
body {{ margin: 0; padding: 16px; background: #10151c; color: #e9edf2;
       font-family: ui-sans-serif, system-ui, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; }}
.card {{ max-width: 34rem; border: 1px solid #2b3b4d; border-radius: 12px;
        background: #161d26; padding: 20px 22px; }}
.watermark {{ font-size: 11px; letter-spacing: .14em; text-transform: uppercase;
             color: #8aa0b4; margin: 0 0 10px; }}
h1 {{ font-size: 1.05rem; margin: 0 0 4px; line-height: 1.35; }}
.doc {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
       font-size: .8rem; color: #8aa0b4; margin: 0 0 14px; }}
.badge {{ display: inline-block; border-radius: 999px; padding: 4px 12px;
         font-size: .75rem; font-weight: 600; letter-spacing: .04em;
         background: {badge_background}; color: {badge_text}; }}
dl {{ display: grid; grid-template-columns: 8.5rem 1fr; gap: 6px 14px; margin: 16px 0 0; }}
dt {{ color: #8aa0b4; font-size: .78rem; }}
dd {{ margin: 0; font-size: .85rem; overflow-wrap: anywhere; }}
.hash {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: .74rem; }}
.detail {{ margin: 16px 0 0; font-size: .78rem; color: #b9c6d4; line-height: 1.5; }}
</style>
</head>
<body>
<div class="card">
  <p class="watermark">{watermark} · {project}</p>
  <h1>{title}</h1>
  <p class="doc">{doc_id} · {path}</p>
  <span class="badge">signature: {status}</span>
  <dl>
    <dt>Issuer</dt><dd>{issuer} <span class="doc">({issuer_id})</span></dd>
    <dt>Dated</dt><dd>{date}</dd>
    <dt>Content hash</dt><dd class="hash">sha256 {hash}</dd>
  </dl>
  <p class="detail">{detail}</p>
</div>
</body>
</html>
"#,
        doc_id = escape_html(&evidence.doc_id),
        path = escape_html(&evidence.path),
        title = escape_html(&evidence.title),
        issuer = escape_html(&evidence.issuer_name),
        issuer_id = escape_html(&evidence.issuer_id),
        project = escape_html(project_name),
        watermark = WATERMARK,
        date = escape_html(date.unwrap_or("not stated in the project record")),
        hash = escape_html(&hash),
        detail = escape_html(&detail),
        status = status,
    )
}

/// Escape the five characters that can break out of HTML text or an attribute.
fn escape_html(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{record::Visibility, verify::DocumentStatus};

    fn evidence(status: DocumentStatus, detail: &str) -> DocumentEvidence {
        DocumentEvidence {
            doc_id: "CERT-FIRE-2026-087".to_owned(),
            title: "Fire & life-safety compliance certificate".to_owned(),
            path: "docs/certificate-fire-life-safety-CERT-FIRE-2026-087.md".to_owned(),
            issuer_id: "northwind-fire-safety-institute".to_owned(),
            issuer_name: "Northwind <Fire> Safety Institute".to_owned(),
            visibility: Visibility::Public,
            sha256: Some("ab".repeat(32)),
            status,
            detail: detail.to_owned(),
        }
    }

    #[test]
    fn a_card_uri_round_trips() {
        let uri = card_uri("CERT-FIRE-2026-087");
        assert_eq!(uri, "ui://ask-aunova/evidence/CERT-FIRE-2026-087");
        assert_eq!(doc_id_from_uri(&uri), Some("CERT-FIRE-2026-087"));
    }

    #[test]
    fn anything_that_is_not_an_evidence_card_uri_is_rejected() {
        assert_eq!(doc_id_from_uri("ui://ask-aunova/evidence/"), None);
        assert_eq!(doc_id_from_uri("file:///etc/passwd"), None);
        assert_eq!(doc_id_from_uri("ui://somewhere/else/CERT"), None);
        assert_eq!(doc_id_from_uri(""), None);
    }

    #[test]
    fn a_verified_card_states_issuer_date_hash_and_status() {
        let card = render(
            &evidence(DocumentStatus::Verified, ""),
            Some("2026-07-18"),
            "Palm Vista Tower",
        );
        assert!(card.contains("signature: verified"), "{card}");
        assert!(card.contains("2026-07-18"), "{card}");
        assert!(card.contains(&"ab".repeat(32)), "{card}");
        assert!(
            card.contains("Northwind &lt;Fire&gt; Safety Institute"),
            "{card}"
        );
        assert!(card.contains(WATERMARK), "{card}");
    }

    #[test]
    fn a_broken_card_says_which_check_failed_and_never_says_verified() {
        let card = render(
            &evidence(
                DocumentStatus::SignatureInvalid,
                "hash: CERT-FIRE-2026-087 (docs/x.md) — the manifest records sha256 aa, but the \
                 file on disk hashes to bb",
            ),
            Some("2026-07-18"),
            "Palm Vista Tower",
        );
        assert!(card.contains("signature: signature_invalid"), "{card}");
        assert!(card.contains("the file on disk hashes to bb"), "{card}");
        assert!(
            !card.contains("signature: verified"),
            "a broken card must never render as verified: {card}",
        );
    }

    #[test]
    fn a_card_fetches_nothing_from_the_network() {
        let card = render(
            &evidence(DocumentStatus::Verified, ""),
            None,
            "Palm Vista Tower",
        );
        for forbidden in ["http://", "https://", "//cdn", "<script", "@import", "src="] {
            assert!(
                !card.contains(forbidden),
                "an evidence card must be self-contained, found `{forbidden}`: {card}",
            );
        }
        assert!(card.contains("not stated in the project record"), "{card}");
    }

    #[test]
    fn markup_in_a_document_title_cannot_escape_the_card() {
        let mut broken = evidence(DocumentStatus::Verified, "");
        broken.title = "<img onerror=\"alert('x')\">".to_owned();
        let card = render(&broken, None, "Palm Vista Tower");
        assert!(!card.contains("<img"), "{card}");
        assert!(
            card.contains("&lt;img onerror=&quot;alert(&#39;x&#39;)&quot;&gt;"),
            "{card}"
        );
    }
}
