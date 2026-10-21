//! Speaking an answer out loud.
//!
//! [`SilentVoice`] is what everything runs on: it produces no audio and says
//! so, rather than pretending. The trait exists so a hosted text-to-speech
//! service can be dropped in behind it.
//!
//! Speaking cannot fail the turn. A voice returns what it managed to do — the
//! words always, audio when it produced any, and a note saying what happened —
//! so an answer is never lost because synthesis was unavailable. The page
//! falls back to showing the words, which is the behaviour you want on stage
//! anyway.

use std::{future::Future, pin::Pin};

use serde::Serialize;

#[cfg(feature = "live")]
pub mod live;

/// Audio a voice produced, ready for the page to play.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AudioClip {
    /// The media type, for the page's `<audio>` element.
    pub media_type: String,
    /// The audio itself, base64-encoded.
    pub base64: String,
}

/// What a voice did with an answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Speech {
    /// The words a listener would hear.
    pub text: String,
    /// Audio to play, when this voice produced any.
    pub audio: Option<AudioClip>,
    /// What this voice did, in one sentence, for the page to show.
    pub note: String,
}

/// The future a [`Voice`] returns.
///
/// Mirrors [`Transcribing`](crate::transcribe::Transcribing) and
/// [`Stepping`](crate::model::Stepping): `async fn` in a trait is not
/// dyn-compatible, and this crate holds its implementations behind
/// `Box<dyn …>`.
pub type Speaking<'a> = Pin<Box<dyn Future<Output = Speech> + Send + 'a>>;

/// Turns an answer into speech.
pub trait Voice: Send + Sync {
    /// What to call this voice in the page.
    fn name(&self) -> &str;

    /// Whether the text to be spoken leaves this machine.
    ///
    /// The answer is composed here, so a hosted voice means the finished
    /// answer is sent to a speech service. That is a smaller disclosure than
    /// sending somebody's recorded voice, and it is still one the page has to
    /// be able to make truthfully.
    fn sends_text_off_this_machine(&self) -> bool;

    /// Speak `text`. Never fails the turn: a voice that could not synthesise
    /// returns the words with no audio and a note saying why.
    fn speak<'a>(&'a self, text: &'a str) -> Speaking<'a>;
}

/// The voice this build runs on: it synthesises nothing and says so.
#[derive(Clone, Copy, Debug, Default)]
pub struct SilentVoice;

impl Voice for SilentVoice {
    fn name(&self) -> &str {
        "silent"
    }

    fn sends_text_off_this_machine(&self) -> bool {
        false
    }

    fn speak<'a>(&'a self, text: &'a str) -> Speaking<'a> {
        Box::pin(async move {
            Speech {
                text: text.to_owned(),
                audio: None,
                note: "This simulator's voice synthesises no audio. The answer above is the text \
                       a speech service would be given, word for word."
                    .to_owned(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_silent_voice_returns_the_words_and_no_audio() {
        let spoken = SilentVoice.speak("Milestone MS-07 is in progress.").await;
        assert_eq!(spoken.text, "Milestone MS-07 is in progress.");
        assert_eq!(spoken.audio, None);
        assert!(
            spoken.note.contains("synthesises no audio"),
            "the stub must say it produced nothing: {}",
            spoken.note,
        );
    }

    #[tokio::test]
    async fn the_silent_voice_does_not_alter_the_words() {
        // Whatever the loop composed is what a speech service would be given.
        // A voice that edited the text would be a second place facts could
        // change after the honesty check.
        let answer = "I can't verify that certificate. [CERT-MEP-2026-052]";
        assert_eq!(SilentVoice.speak(answer).await.text, answer);
    }
}
