//! The one page this binary serves.
//!
//! It is compiled into the binary, so running the simulator needs no asset
//! directory and cannot be served a different page than the one that was
//! tested. Everything it needs — styles, script, layout — is inline: there is
//! no stylesheet, no font, no script and no image fetched from anywhere, which
//! the tests below hold it to by refusing any `://` in the file at all.
//!
//! The browser therefore talks to exactly one host, this binary on loopback,
//! and this binary talks to exactly one host, the MCP server on loopback.

/// The simulator page, inlined at compile time.
pub const PAGE_HTML: &str = include_str!("page.html");

/// What the page is served as.
pub const PAGE_CONTENT_TYPE: &str = "text/html; charset=utf-8";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_fetches_nothing_from_anywhere() {
        // Not "no CDN we happen to know about": no absolute reference of any
        // kind. A scheme separator is the one thing every remote reference
        // has, so its absence is the whole claim in one assertion.
        assert!(
            !PAGE_HTML.contains("://"),
            "the page must carry no absolute URL; the browser talks only to this binary",
        );
        for remote in [
            "cdn",
            "googleapis",
            "unpkg",
            "jsdelivr",
            "@import",
            "integrity=",
        ] {
            assert!(
                !PAGE_HTML.contains(remote),
                "the page must not reach for `{remote}`",
            );
        }
    }

    #[test]
    fn the_page_says_plainly_that_it_is_a_simulator() {
        assert!(PAGE_HTML.contains("<b>Simulator</b>"));
        assert!(
            PAGE_HTML.contains("not a voice-assistant"),
            "the label has to say what it is not, not only what it is",
        );
        assert!(PAGE_HTML.contains("not any vendor"));
    }

    #[test]
    fn the_page_carries_no_assistant_product_branding() {
        // Named here only so their absence is checked rather than asserted.
        // The page is a simulator of the idea, not of anybody's product.
        for brand in [
            "Alexa",
            "Siri",
            "Cortana",
            "Bixby",
            "Google Assistant",
            "Echo Dot",
        ] {
            assert!(
                !PAGE_HTML.contains(brand),
                "the page must carry no assistant product branding, and it names `{brand}`",
            );
        }
    }

    #[test]
    fn the_page_claims_no_authority_endorsement() {
        assert!(PAGE_HTML.contains("synthetic demonstration data for a fictional project"));
        assert!(PAGE_HTML.contains("no real project, regulator or authority is involved"));
        assert!(PAGE_HTML.contains("approved or endorsed by anyone"));
    }

    #[test]
    fn the_typed_path_works_whether_or_not_there_is_a_recogniser() {
        assert!(PAGE_HTML.contains("id=\"question\""));
        assert!(PAGE_HTML.contains("id=\"ask\""));
        assert!(
            PAGE_HTML.contains("if (!Recogniser)"),
            "push-to-talk must degrade rather than break the page",
        );
        assert!(PAGE_HTML.contains("Type the question instead"));
    }

    #[test]
    fn the_evidence_cards_are_rendered_sandboxed() {
        // A card is HTML the page did not write. It goes in an iframe with an
        // empty sandbox, which is no scripts, no forms and no same-origin.
        assert!(PAGE_HTML.contains("frame.setAttribute(\"sandbox\", \"\")"));
    }

    #[test]
    fn the_page_carries_both_capture_paths() {
        // The browser's own recogniser is the default where there is one,
        // and every alternative it offers is read rather than its best.
        assert!(PAGE_HTML.contains("window.webkitSpeechRecognition"));
        assert!(PAGE_HTML.contains("recogniser.maxAlternatives = 5"));

        // The other path captures the samples here: 16 kHz mono signed
        // 16-bit PCM, which is what a streaming speech service takes as it
        // stands. No container, no codec, nothing to decode at either end.
        assert!(PAGE_HTML.contains("audioWorklet.addModule"));
        assert!(PAGE_HTML.contains("const TARGET_RATE = 16000;"));
        assert!(PAGE_HTML.contains("pcm.setInt16(index * 2, Math.round(clamped * 32767), true);"));
        assert!(
            !PAGE_HTML.contains("MediaRecorder"),
            "capturing a container would put a decoder in the live path",
        );

        // And it is selectable, as well as being what a failed recogniser
        // falls back to.
        assert!(PAGE_HTML.contains("id=\"capture-toggle\""));
        assert!(PAGE_HTML.contains("state.capture = \"simulator\";"));
    }

    #[test]
    fn both_capture_paths_end_at_the_same_normaliser_and_the_same_confirmation() {
        // Two producers, one interpretation path: the recogniser's words go
        // to `/api/interpret`, the recording goes to `/api/transcribe`, and
        // both are shown by the same function before anything is asked.
        assert!(PAGE_HTML.contains("fetch(\"/api/interpret\""));
        assert!(PAGE_HTML.contains("fetch(\"/api/transcribe\""));
        assert_eq!(
            PAGE_HTML.matches("showHeard(").count(),
            3,
            "both capture paths, and nothing else, must go through the panel that shows what \
             was heard",
        );
    }

    #[test]
    fn no_capture_path_asks_anything_without_a_person_confirming_it() {
        // `ask()` is reachable from exactly three places, and a recognition
        // is not one of them: the Ask button, the scripted chips, and the
        // confirm button under the transcript. If this count changes, a
        // fourth way to reach a tool call was added, and it has to be one a
        // person pressed.
        assert_eq!(
            PAGE_HTML.matches("ask();").count(),
            3,
            "a question must be asked only by a chip, the Ask button or the confirmation",
        );
        assert!(PAGE_HTML.contains("id=\"confirm\""));
        assert!(PAGE_HTML.contains("el(\"confirm\").disabled = true;"));
        assert!(
            PAGE_HTML.contains("What was heard &mdash; nothing is asked until you confirm it"),
            "the panel has to say that it is a confirmation",
        );
    }

    #[test]
    fn a_recogniser_that_fails_says_so_rather_than_switching_quietly() {
        assert!(PAGE_HTML.contains("This browser's speech recogniser stopped: "));
        assert!(PAGE_HTML.contains("Push-to-talk has switched to recording audio here"));
    }

    #[test]
    fn the_page_says_where_a_recorded_voice_actually_goes() {
        // "Loopback only" is true of typing and false of both voice paths.
        // A page that says the first while doing the second is exactly the
        // kind of true-sounding claim this project exists to refuse.
        assert!(PAGE_HTML.contains("<b>Where what you say goes.</b>"));
        assert!(PAGE_HTML.contains("<b>Push-to-talk is separate\n    again.</b>"));
        assert!(
            PAGE_HTML.contains("in Chrome, that is Google"),
            "the browser path sends audio to the browser's own speech service, and the page \
             has to name it",
        );
        assert!(
            PAGE_HTML.contains("would your audio be sent to Amazon Transcribe"),
            "the recording path's live destination has to be named too",
        );
        assert!(
            PAGE_HTML.contains("--live-transcribe"),
            "and the page has to name the switch that would send it, since `--live` alone does \
             not and a reader otherwise cannot tell which of the two they are running",
        );
        assert!(
            PAGE_HTML.contains("<span id=\"recording-destination\"></span>"),
            "the footer's sentence about where a recording goes has to be filled in at runtime; \
             as static text it said the audio went nowhere in the one build where it does go \
             somewhere",
        );
        assert!(
            PAGE_HTML.contains("el(\"recording-destination\").textContent = state.liveTranscriber"),
            "and it has to come from the server's own flag, like the other two lines",
        );
        // The claim may live only in a guarded branch of the script, never
        // in the markup. Counting occurrences across the whole page was the
        // wrong rule and failed against a correct fix: both remaining ones
        // are branches of `state.liveTranscriber`, which is the point.
        let markup = PAGE_HTML
            .split_once("<script")
            .expect("the page has a script block")
            .0;
        assert!(
            !markup.contains("sends it nowhere"),
            "the footer said the recording goes nowhere as static text, which is false in a \
             --live-transcribe build; it has to be filled in from the flag",
        );
        assert!(
            PAGE_HTML.contains("function captureDestination("),
            "and which one is active has to be said beside the button, before it is pressed",
        );
    }

    #[test]
    fn the_page_names_every_hosted_service_actually_in_use() {
        // "Talks only to the MCP server on loopback" is true of a default
        // build and false the moment a hosted model or voice is switched on,
        // so the page cannot state it once and leave it there. The footer
        // carries a line built from the server's own flags.
        assert!(
            PAGE_HTML.contains("<p id=\"hosted-note\"></p>"),
            "the footer needs somewhere to say what is live",
        );
        for flag in [
            "about.model_is_live === true",
            "about.voice_is_live === true",
            "about.transcriber_is_live === true",
        ] {
            assert!(
                PAGE_HTML.contains(flag),
                "each seam is disclosed from its own flag, not inferred: {flag} is missing",
            );
        }
        assert!(
            PAGE_HTML.contains("Running on stubs: nothing on this page is sent to any hosted"),
            "and a build that sends nowhere has to say so too, rather than saying nothing",
        );
    }

    #[test]
    fn push_to_talk_only_falls_back_to_a_transcriber_that_can_answer() {
        // The defect this exists for: the recogniser's error handler ticked
        // the capture toggle on *any* error, including "no speech". In every
        // build except `--live --live-transcribe` that routed push-to-talk at
        // the fixture stub, which can only refuse an unknown recording — so a
        // pause while speaking left the button permanently dead.
        assert!(
            PAGE_HTML.contains("if (state.liveTranscriber) {"),
            "switching capture path must be gated on the simulator actually having a \
             transcriber that transcribes, not done unconditionally",
        );
        assert!(
            PAGE_HTML.contains("state.liveTranscriber = about.transcriber_is_live === true;"),
            "and that gate has to come from the server's own answer, not from matching the \
             prose in `transcriber`",
        );
        assert!(
            PAGE_HTML.contains("event.error === \"no-speech\" || event.error === \"aborted\""),
            "a press that caught nothing is not a broken recogniser and must not switch paths",
        );

        // Scoped to the error handler on purpose. Elsewhere the page does
        // switch to the recording path unconditionally — when the browser
        // exposes no recogniser at all, where it is the only voice path there
        // is. The defect was switching on *error*, so that is the region the
        // guard has to hold in.
        let handler = PAGE_HTML
            .split_once("recogniser.addEventListener(\"error\"")
            .expect("the recogniser has an error handler")
            .1
            .split_once("recogniser.addEventListener(\"end\"")
            .expect("the error handler ends before the end handler")
            .0;
        assert_eq!(
            handler.matches("state.capture = \"simulator\";").count(),
            1,
            "the error handler may switch path in exactly one place, the guarded one",
        );
        assert!(
            handler.contains("if (state.liveTranscriber) {"),
            "and that one place is inside the guard",
        );

        // The unknown case needs its own branch and its own words. With a
        // single `else` it told somebody whose `/api/about` had failed that
        // the other path transcribes only fixtures and there is nowhere to
        // switch to — a definite statement about a thing the page had not
        // been able to find out.
        let unknown = handler
            .split_once("} else if (state.liveTranscriber === null) {")
            .expect("the error handler distinguishes an unknown transcriber from a stub one")
            .1
            .split_once("} else {")
            .expect("and the unknown branch ends before the stub one")
            .0;
        assert!(
            !unknown.contains("nowhere"),
            "the unknown branch claims there is nowhere to switch to, which is the one thing \
             it does not know: {unknown}",
        );
        assert!(
            unknown.contains("could not ask the simulator"),
            "and it has to say why it cannot tell them: {unknown}",
        );
    }

    #[test]
    fn money_is_shown_in_the_form_it_is_said_and_a_contradiction_is_marked() {
        // `147000000` is a count of fils. The statement list shows the said
        // form; the drawer below it still shows the server's own result.
        assert!(PAGE_HTML.contains("text.textContent = statement.said;"));
        assert!(PAGE_HTML.contains("ul.statements li.contradicted"));
    }
}
