//! Speaking an answer with Amazon Polly.
//!
//! **This is one of the two paths in this crate that leaves the machine, and
//! nothing in the default build constructs it.** It compiles only under the
//! `live` feature and is reached only when the binary is started with
//! `--live`; every test runs on [`SilentVoice`](super::SilentVoice).
//!
//! **Speaking cannot fail the turn**, which is the whole reason this is not a
//! `Result`. A synthesis that fails returns the words with no audio and a note
//! saying what happened, so an answer is never lost because a hosted service
//! was unavailable. On stage that degrades to reading the answer on screen,
//! which is the behaviour you want anyway.
//!
//! **No SDK error is ever formatted into the note.** An SDK error's `Display`
//! can carry the request URL and, through it, query parameters; the failures
//! here are classified into fixed phrases instead. The detail goes to the log.

use aws_sdk_polly::{
    Client,
    types::{Engine, OutputFormat, VoiceId},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::voice::{AudioClip, Speaking, Speech, Voice};

/// The media type Polly returns for `Mp3`, and what the page's `<audio>`
/// element is given.
const MEDIA_TYPE: &str = "audio/mpeg";

/// A voice that reads the answer with Amazon Polly's neural engine.
#[derive(Clone, Debug)]
pub struct PollyVoice {
    client: Client,
    voice_id: VoiceId,
}

impl PollyVoice {
    /// Build a voice from the shared profile configuration.
    ///
    /// Constructing a client makes no request: credentials are resolved when
    /// the first call is made, not here.
    pub async fn new() -> Result<Self, crate::aws::AwsError> {
        let config = crate::aws::config().await?;
        Ok(Self {
            client: Client::new(&config),
            voice_id: VoiceId::Joanna,
        })
    }
}

impl Voice for PollyVoice {
    fn name(&self) -> &str {
        "Amazon Polly, neural engine"
    }

    fn sends_text_off_this_machine(&self) -> bool {
        true
    }

    fn speak<'a>(&'a self, text: &'a str) -> Speaking<'a> {
        Box::pin(async move {
            let spoken = text.to_owned();
            let note = match self.synthesise(text).await {
                Ok(audio) => {
                    return Speech {
                        text: spoken,
                        audio: Some(audio),
                        note: "Spoken by Amazon Polly's neural engine.".to_owned(),
                    };
                }
                Err(reason) => reason,
            };
            Speech {
                text: spoken,
                audio: None,
                note,
            }
        })
    }
}

impl PollyVoice {
    /// One synthesis, or a fixed phrase saying what went wrong.
    async fn synthesise(&self, text: &str) -> Result<AudioClip, String> {
        let spoken = self
            .client
            .synthesize_speech()
            .engine(Engine::Neural)
            .output_format(OutputFormat::Mp3)
            .voice_id(self.voice_id.clone())
            .text(text)
            .send()
            .await
            .map_err(|error| {
                // Logged in full for the operator; never returned to the page.
                // `DisplayErrorContext`, because an `SdkError`'s own Display
                // is "service error" and names neither the code nor the
                // message.
                tracing::warn!(
                    "Polly refused the synthesis: {}",
                    aws_smithy_types::error::display::DisplayErrorContext(&error)
                );
                "Amazon Polly did not synthesise this answer. The words are above; \
                 what went wrong is recorded where this simulator is run."
                    .to_owned()
            })?;

        let bytes = spoken
            .audio_stream
            .collect()
            .await
            .map_err(|error| {
                tracing::warn!(
                    "Polly's audio stream did not finish: {}",
                    aws_smithy_types::error::display::DisplayErrorContext(&error)
                );
                "Amazon Polly started speaking and the audio did not finish arriving. \
                 The words are above."
                    .to_owned()
            })?
            .into_bytes();

        if bytes.is_empty() {
            return Err(
                "Amazon Polly returned no audio for this answer. The words are above.".to_owned(),
            );
        }

        Ok(AudioClip {
            media_type: MEDIA_TYPE.to_owned(),
            base64: STANDARD.encode(&bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_type_is_the_one_the_page_can_play() {
        // `OutputFormat::Mp3` is requested above; these two have to agree, and
        // nothing else in the crate ties them together.
        assert_eq!(MEDIA_TYPE, "audio/mpeg");
        assert_eq!(OutputFormat::Mp3.as_str(), "mp3");
    }
}
