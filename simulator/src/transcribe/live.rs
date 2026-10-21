//! The live transcriber: `StartStreamTranscription`, and nothing else.
//!
//! This module is compiled only under the `live` feature, which is off by
//! default, and even then [`StreamingTranscriber`] is constructed only when
//! the binary is started with `--live-transcribe` on top of `--live`. No
//! test constructs it and no default path reaches it. Two switches, because
//! it is the one path here that sends a person's recorded voice off the
//! machine, and that is a decision to take deliberately rather than a
//! default to fall into.
//!
//! **Streaming only.** `StartStreamTranscription` opens one HTTP/2 stream,
//! sends the samples as audio events and reads transcripts back. The batch
//! API — `StartTranscriptionJob` — is not used and is not reachable from
//! here: it requires putting the recording in an S3 bucket first, which turns
//! a question somebody asked out loud into an object someone else is storing.
//!
//! **What it sends.** The samples exactly as the page captured them: 16 kHz
//! mono signed 16-bit little-endian PCM, which is a media encoding Transcribe
//! accepts as it stands. No container is written and no codec runs, here or
//! in the browser, so this repository has no audio codec in its dependency
//! tree and nothing to keep patched.
//!
//! **Configuration.** Region and credentials come from [`crate::aws`], built
//! once and shared with the live model and the live voice: the shared
//! profile named by `AWS_PROFILE`, with the credentials provider pinned to
//! it rather than taken from the SDK's default chain, which would prefer an
//! exported key pair. This crate holds no access key, no secret and no
//! session token of its own, and has no code path that could log one: the
//! only thing it names is the profile. A missing `AWS_PROFILE` is a
//! [`TranscribeError::NotConfigured`] before any request is made.
//!
//! **What it never does.** No credential appears in an error, a log line or a
//! panic message: an SDK error's `Display` can reach a request URL and a
//! service message, so failures are classified into fixed phrases the same
//! way [`crate::mcp`] classifies transport errors.

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use aws_sdk_transcribestreaming::{
    Client,
    primitives::Blob,
    types::{
        AudioEvent, AudioStream, LanguageCode, MediaEncoding, TranscriptEvent,
        TranscriptResultStream, error::AudioStreamError,
    },
};
use futures_core::Stream;

use crate::transcribe::{
    Recording, SAMPLE_RATE_HZ, TranscribeError, Transcriber, Transcribing, Transcript,
};

/// What this transcriber is called in an error, and in the page.
const SERVICE: &str = "Amazon Transcribe streaming";

/// Environment variable naming the shared profile to use.
pub use crate::aws::PROFILE_ENV;

/// How much audio goes in one event.
///
/// A tenth of a second at the capture rate. Small enough that the service is
/// fed steadily rather than in one lump, large enough that a short question
/// is a handful of frames.
const CHUNK_BYTES: usize = (SAMPLE_RATE_HZ as usize / 10) * 2;

/// How long one transcription may take before it is abandoned.
///
/// A bounded read of an unbounded stream: the service could keep a stream
/// open, and a page waiting on it is a page that never answers.
const DEADLINE: Duration = Duration::from_secs(60);

/// Most transcript events one recording may produce.
///
/// The recording is capped at thirty seconds and the service emits results as
/// it goes, so this is far above what any one of them can need. It exists so
/// that a stream which never ends is still a failed transcription rather than
/// a loop.
const MAX_EVENTS: usize = 2_000;

/// Longest transcript this will carry back, in characters.
const MAX_TRANSCRIPT_CHARS: usize = 2_000;

/// A transcriber that calls the live streaming service.
#[derive(Debug)]
pub struct StreamingTranscriber {
    client: Client,
    /// The language the page is captioned in and the recogniser is asked for.
    language: LanguageCode,
}

impl StreamingTranscriber {
    /// Build a client from the shared profile configuration.
    ///
    /// Nothing is sent here: constructing a client resolves no credentials,
    /// so a profile that exists but cannot produce them fails at the first
    /// call, where the SDK can say which provider refused.
    pub async fn new(language: LanguageCode) -> Result<Self, TranscribeError> {
        let config = crate::aws::config()
            .await
            .map_err(|_| TranscribeError::NotConfigured {
                service: SERVICE,
                variable: PROFILE_ENV,
            })?;

        Ok(Self {
            client: Client::new(&config),
            language,
        })
    }

    /// Stream one recording and collect the transcripts it comes back with.
    async fn stream(&self, recording: &Recording) -> Result<Transcript, TranscribeError> {
        let audio = AudioChunks {
            pcm: recording.pcm().to_vec(),
            sent: 0,
        };

        let started = self
            .client
            .start_stream_transcription()
            .language_code(self.language.clone())
            .media_encoding(MediaEncoding::Pcm)
            .media_sample_rate_hertz(i32::try_from(SAMPLE_RATE_HZ).map_err(|_| {
                TranscribeError::Service {
                    service: SERVICE,
                    detail: "the capture rate does not fit the field the service takes",
                }
            })?)
            .audio_stream(audio.into())
            .send()
            .await
            .map_err(|sdk_error| {
                // Logged for the operator, never returned: an SDK error
                // renders the request URI and the service's own message, and
                // a signed URI is a credential. `DisplayErrorContext` is what
                // turns "service error" into the code that names the cause.
                tracing::warn!(
                    "{SERVICE} refused the stream: {}",
                    aws_smithy_types::error::display::DisplayErrorContext(&sdk_error)
                );
                TranscribeError::Service {
                    service: SERVICE,
                    detail: "the stream could not be started",
                }
            })?;

        let mut results = started.transcript_result_stream;
        let mut alternatives: Vec<String> = Vec::new();

        for _event in 0..MAX_EVENTS {
            let received = tokio::time::timeout(DEADLINE, results.recv())
                .await
                .map_err(|_elapsed| TranscribeError::Service {
                    service: SERVICE,
                    detail: "it did not finish the transcript within the deadline",
                })?
                .map_err(|sdk_error| {
                    tracing::warn!(
                        "{SERVICE} stopped the transcript stream: {}",
                        aws_smithy_types::error::display::DisplayErrorContext(&sdk_error)
                    );
                    TranscribeError::Service {
                        service: SERVICE,
                        detail: "the transcript stream stopped before it was complete",
                    }
                })?;

            let Some(event) = received else {
                return Ok(Transcript {
                    alternatives,
                    note: format!(
                        "{SERVICE}: the recording was streamed to the service as 16 kHz mono \
                         PCM, and this is what came back."
                    ),
                });
            };

            // One variant is read and the rest are not: a later revision of
            // the service may add events this was not written for, and an
            // event nobody read is better than a sentence nobody said.
            if let TranscriptResultStream::TranscriptEvent(transcribed) = event {
                collect(transcribed, &mut alternatives);
            }
        }

        Err(TranscribeError::Service {
            service: SERVICE,
            detail: "the transcript stream carried more events than one recording may produce",
        })
    }
}

impl Transcriber for StreamingTranscriber {
    fn name(&self) -> &str {
        "Amazon Transcribe streaming (live): the recording is sent to the service"
    }

    fn sends_recordings_off_this_machine(&self) -> bool {
        true
    }

    fn transcribe<'a>(&'a self, recording: &'a Recording) -> Transcribing<'a> {
        Box::pin(self.stream(recording))
    }
}

/// Take the settled alternatives out of one transcript event.
///
/// Partial results are skipped: the service revises them as more audio
/// arrives, and a half-heard sentence shown as a transcript is a sentence
/// nobody said yet.
fn collect(event: TranscriptEvent, into: &mut Vec<String>) {
    let Some(transcript) = event.transcript else {
        return;
    };
    for result in transcript.results.unwrap_or_default() {
        if result.is_partial {
            continue;
        }
        for alternative in result.alternatives.unwrap_or_default() {
            let Some(heard) = alternative.transcript else {
                continue;
            };
            let heard = heard.trim();
            if heard.is_empty() || heard.chars().count() > MAX_TRANSCRIPT_CHARS {
                continue;
            }
            if !into.iter().any(|kept| kept == heard) {
                into.push(heard.to_owned());
            }
        }
    }
}

/// The recording, as the stream of audio events the service reads.
///
/// A hand-written [`Stream`] rather than a combinator crate: it yields one
/// fixed-size chunk per poll and finishes, which is the whole of it.
struct AudioChunks {
    /// The samples, exactly as they were captured.
    pcm: Vec<u8>,
    /// How many bytes have been handed over.
    sent: usize,
}

impl Stream for AudioChunks {
    type Item = Result<AudioStream, AudioStreamError>;

    fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // `AudioChunks` holds a `Vec` and a `usize`, so it is `Unpin` and
        // this needs no unsafe projection.
        let chunks = self.get_mut();
        if chunks.sent >= chunks.pcm.len() {
            return Poll::Ready(None);
        }
        let until = chunks.pcm.len().min(chunks.sent + CHUNK_BYTES);
        let chunk = chunks.pcm[chunks.sent..until].to_vec();
        chunks.sent = until;
        Poll::Ready(Some(Ok(AudioStream::AudioEvent(
            AudioEvent::builder().audio_chunk(Blob::new(chunk)).build(),
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll a chunk stream to exhaustion without a runtime.
    ///
    /// Every poll of this stream is ready, so no waker is ever used; a
    /// [`Context`] still has to be handed in, and this is the smallest one
    /// that satisfies that.
    fn drain(mut chunks: AudioChunks) -> Vec<usize> {
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut sizes = Vec::new();
        loop {
            match Pin::new(&mut chunks).poll_next(&mut context) {
                Poll::Ready(Some(Ok(AudioStream::AudioEvent(event)))) => {
                    sizes.push(
                        event
                            .audio_chunk()
                            .map(|blob| blob.as_ref().len())
                            .unwrap_or(0),
                    );
                }
                Poll::Ready(None) => return sizes,
                other => panic!("a chunk stream yields chunks until it ends: {other:?}"),
            }
        }
    }

    #[test]
    fn a_recording_is_sent_as_whole_chunks_and_a_last_short_one() {
        let pcm = vec![7u8; CHUNK_BYTES * 2 + 100];
        let sizes = drain(AudioChunks {
            pcm: pcm.clone(),
            sent: 0,
        });
        assert_eq!(sizes, vec![CHUNK_BYTES, CHUNK_BYTES, 100]);
        assert_eq!(
            sizes.iter().sum::<usize>(),
            pcm.len(),
            "every byte recorded is a byte sent, and no byte is sent twice",
        );
    }

    #[test]
    fn an_empty_recording_sends_nothing() {
        assert!(
            drain(AudioChunks {
                pcm: Vec::new(),
                sent: 0,
            })
            .is_empty()
        );
    }

    #[tokio::test]
    async fn a_missing_profile_is_named_and_nothing_is_sent() {
        // The profile is the whole of this transcriber's configuration, and
        // an unnamed one must be a refusal before a request rather than an
        // error from the service.
        if std::env::var(PROFILE_ENV).is_ok() {
            return;
        }
        let error = StreamingTranscriber::new(LanguageCode::EnGb)
            .await
            .expect_err("an unnamed profile must refuse");
        let message = error.to_string();
        assert!(message.contains("is not configured"), "{message}");
        assert!(message.contains("no request was made"), "{message}");
    }

    #[test]
    fn a_chunk_is_a_tenth_of_a_second_of_the_capture_format() {
        assert_eq!(CHUNK_BYTES, 3_200);
        assert_eq!(CHUNK_BYTES % crate::transcribe::BYTES_PER_SAMPLE, 0);
    }
}
