//! Turning a recording into words.
//!
//! The page captures the microphone itself and sends this binary raw PCM:
//! **16 kHz, mono, signed 16-bit little-endian**, and nothing else. That is
//! not an arbitrary choice. It is the one format a streaming speech service
//! takes as it stands, so no container is parsed and no codec is decoded
//! anywhere in this repository — there is no WebM to remux, no Opus to
//! unpack, and so no audio-codec dependency to vet, update or be bitten by.
//! The browser's `AudioWorklet` already has the samples in this form; all it
//! does is resample and narrow them to 16-bit before the POST.
//!
//! [`StubTranscriber`] is what every test and every default path runs on. It
//! recognises the fixture recordings this crate generates and nothing else: a
//! recording it does not know is a [`TranscribeError::NoFixture`], said
//! plainly, rather than a plausible sentence nobody uttered. A stub that
//! invented a transcript would be the same failure the whole project argues
//! against, one layer lower down.
//!
//! The fixtures are generated rather than recorded — a square wave at a
//! distinct frequency each, computed in integer arithmetic so the bytes are
//! identical on every machine. `tests/voice.rs` holds the committed `.pcm`
//! files to the generator, so the fixture a test posts is the fixture this
//! module recognises.

use std::{future::Future, pin::Pin};

use serde::Serialize;

/// The live streaming transcriber, when this build was asked for one.
///
/// Off by default, and nothing in this crate constructs it. See the module's
/// own documentation for what it sends, and where.
#[cfg(feature = "live")]
pub mod live;

/// The sample rate this simulator captures and transcribes at.
///
/// 16 kHz is what `StartStreamTranscription` takes for PCM, and what the
/// page's worklet resamples to.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// Bytes per sample: signed 16-bit little-endian, one channel.
pub const BYTES_PER_SAMPLE: usize = 2;

/// Longest recording this simulator will accept, in bytes.
///
/// Thirty seconds at the rate above. A spoken question is a few seconds; the
/// cap exists so an unbounded body cannot be posted at this binary, and it is
/// *detected* rather than silently truncated — a recording past it is refused
/// whole, so nobody is answered a question they did not finish asking.
pub const MAX_RECORDING_BYTES: usize = SAMPLE_RATE_HZ as usize * BYTES_PER_SAMPLE * 30;

/// How loud a generated fixture is, as a sample value.
///
/// About a third of full scale: plainly audible if anyone plays one, and far
/// from clipping.
const FIXTURE_AMPLITUDE: i16 = 8_000;

/// A recording, in the one format this simulator accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recording {
    pcm: Vec<u8>,
}

impl Recording {
    /// Take bytes from a caller, or say why they are not a recording.
    ///
    /// Every bound is checked here, once, so no transcriber has to: an odd
    /// byte count is not 16-bit samples, and silently dropping the last byte
    /// would be this crate deciding what the person meant.
    pub fn new(pcm: Vec<u8>) -> Result<Self, TranscribeError> {
        if pcm.is_empty() {
            return Err(TranscribeError::Empty);
        }
        if pcm.len() > MAX_RECORDING_BYTES {
            return Err(TranscribeError::TooLong { bytes: pcm.len() });
        }
        if !pcm.len().is_multiple_of(BYTES_PER_SAMPLE) {
            return Err(TranscribeError::NotAligned { bytes: pcm.len() });
        }
        Ok(Self { pcm })
    }

    /// The samples, as they arrived.
    pub fn pcm(&self) -> &[u8] {
        &self.pcm
    }
}

/// What was heard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Transcript {
    /// Every alternative, best first. One is a transcriber that offers one.
    pub alternatives: Vec<String>,
    /// What produced this, in one sentence, for the page to show.
    pub note: String,
}

/// Why a recording produced no words.
#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    /// Nothing was sent.
    #[error("the recording is empty, so there is nothing to transcribe")]
    Empty,
    /// Past the cap, and refused whole rather than cut down.
    #[error(
        "a recording may be at most {MAX_RECORDING_BYTES} bytes of 16 kHz mono PCM; this one is \
         {bytes}, so none of it was transcribed"
    )]
    TooLong {
        /// How many bytes arrived.
        bytes: usize,
    },
    /// Not a whole number of 16-bit samples.
    #[error(
        "a recording is signed 16-bit little-endian samples, so its length must be even; this \
         one is {bytes} bytes"
    )]
    NotAligned {
        /// How many bytes arrived.
        bytes: usize,
    },
    /// The stub was handed something that is not one of its fixtures.
    #[error(
        "this build transcribes with a stub that knows only its own fixture recordings, and \
         these {bytes} bytes are not one of them; nothing was transcribed and nothing was \
         guessed. Untick \"Record here and let this simulator transcribe it\" to use the \
         browser's own recogniser, or type the question"
    )]
    NoFixture {
        /// How many bytes arrived.
        bytes: usize,
    },
    /// A live transcriber was asked for without the configuration it needs.
    #[error("{service} is not configured: {variable} is unset, so no request was made")]
    NotConfigured {
        /// Which transcriber wanted it.
        service: &'static str,
        /// The environment variable that is missing.
        variable: &'static str,
    },
    /// A live transcriber could not complete the exchange.
    ///
    /// Classified rather than formatted: an SDK error's `Display` can reach a
    /// request URL, and a request URL can carry a credential.
    #[error("{service} did not transcribe the recording: {detail}")]
    Service {
        /// Which transcriber was asked.
        service: &'static str,
        /// Which kind of failure it was, in fixed words.
        detail: &'static str,
    },
}

/// The future a [`Transcriber`] returns.
///
/// `async fn` in a trait is not dyn-compatible, and this crate holds its
/// implementations behind `Box<dyn …>` exactly as it holds
/// [`Model`](crate::model::Model) and [`Voice`](crate::voice::Voice). Boxing
/// the future is what that costs here; the alternative is a macro crate added
/// to the dependency tree to write the same type out.
pub type Transcribing<'a> =
    Pin<Box<dyn Future<Output = Result<Transcript, TranscribeError>> + Send + 'a>>;

/// Turns a recording into words.
pub trait Transcriber: Send + Sync {
    /// What to call this transcriber in the page.
    fn name(&self) -> &str;

    /// Whether a recording sent here leaves this machine.
    ///
    /// Required rather than defaulted: a new transcriber must answer it, and
    /// a default of `false` would mean the one that forgets is the one that
    /// claims the recording stays put.
    ///
    /// The page needs this for two separate reasons, both load bearing. It is
    /// a disclosure — somebody recording their voice is entitled to know
    /// where it goes. It is also the difference between a working fallback
    /// and a dead one: switching push-to-talk to this path when the
    /// transcriber is a fixture table sends audio somewhere that can only
    /// refuse it.
    fn sends_recordings_off_this_machine(&self) -> bool;

    /// Transcribe one recording, or say why it could not be transcribed.
    ///
    /// Unlike [`Voice::speak`](crate::voice::Voice::speak), this can fail and
    /// must be allowed to: words nobody said are worse than no words.
    fn transcribe<'a>(&'a self, recording: &'a Recording) -> Transcribing<'a>;
}

/// One recording this build knows, and the words it stands for.
#[derive(Clone, Copy, Debug)]
pub struct Fixture {
    /// The file it is committed as, under `tests/fixtures`.
    pub name: &'static str,
    /// The tone it is, in hertz.
    pub tone_hz: u32,
    /// How long it lasts, in milliseconds.
    pub millis: u32,
    /// What the stub hears in it: the alternatives a recogniser would offer,
    /// best first.
    pub heard: &'static [&'static str],
}

impl Fixture {
    /// How many bytes this fixture is.
    pub const fn byte_len(&self) -> usize {
        self.sample_count() * BYTES_PER_SAMPLE
    }

    /// How many samples this fixture is.
    const fn sample_count(&self) -> usize {
        (SAMPLE_RATE_HZ as usize * self.millis as usize) / 1_000
    }
}

/// Every fixture recording, with the transcript each one stands for.
///
/// Three, because three is what the tests need: a question about the buyer's
/// own unit, the same question about a unit that buyer may not read, and a
/// question about the project with no unit in it at all. Each is a fifth of a
/// second, because it is a fixture and not a recording session.
pub const FIXTURES: [Fixture; 3] = [
    Fixture {
        name: "payment-unit-1204.pcm",
        tone_hz: 440,
        millis: 200,
        heard: &["what is the payment status of unit twelve oh four"],
    },
    Fixture {
        name: "payment-unit-0802.pcm",
        tone_hz: 660,
        millis: 200,
        // Two alternatives that agree about the unit, which is the case a
        // recogniser actually produces: the second misheard the last word
        // and resolves to nothing, so the set of units is still one.
        heard: &[
            "what is the payment status of unit oh eight oh two",
            "what is the payment status of unit oh eight oh too",
        ],
    },
    Fixture {
        name: "project-on-track.pcm",
        tone_hz: 880,
        millis: 200,
        heard: &["is palm vista on track and is the structure certified"],
    },
];

/// The exact bytes of one fixture.
///
/// A square wave in integer arithmetic: no floating point anywhere in it, so
/// the bytes are the same on every machine that builds this crate, which is
/// what lets a committed `.pcm` file be checked against it.
pub fn fixture_pcm(fixture: &Fixture) -> Vec<u8> {
    let half_period = (SAMPLE_RATE_HZ as usize / (2 * fixture.tone_hz.max(1) as usize)).max(1);
    let mut pcm = Vec::with_capacity(fixture.byte_len());
    for index in 0..fixture.sample_count() {
        let sample = if (index / half_period).is_multiple_of(2) {
            FIXTURE_AMPLITUDE
        } else {
            -FIXTURE_AMPLITUDE
        };
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

/// The transcriber this build runs on: a fixture table, and no service call.
#[derive(Clone, Copy, Debug, Default)]
pub struct StubTranscriber;

impl StubTranscriber {
    /// Which fixture this recording is, if it is one.
    ///
    /// Matched on the samples themselves rather than on a file name, because
    /// a file name is not what arrives over HTTP. The length is compared
    /// first, so the bytes of a fixture are only generated when a recording
    /// could be it.
    fn look_up(&self, recording: &Recording) -> Result<Transcript, TranscribeError> {
        let found = FIXTURES.iter().find(|fixture| {
            fixture.byte_len() == recording.pcm().len() && fixture_pcm(fixture) == recording.pcm()
        });
        match found {
            Some(fixture) => Ok(Transcript {
                alternatives: fixture
                    .heard
                    .iter()
                    .map(|words| (*words).to_owned())
                    .collect(),
                note: format!(
                    "Stub transcriber: these samples are the `{}` fixture, and this is the \
                     transcript it stands for. No speech service was called.",
                    fixture.name,
                ),
            }),
            None => Err(TranscribeError::NoFixture {
                bytes: recording.pcm().len(),
            }),
        }
    }
}

impl Transcriber for StubTranscriber {
    fn name(&self) -> &str {
        "fixture stub: a table of known recordings, no speech service and no network call"
    }

    fn sends_recordings_off_this_machine(&self) -> bool {
        false
    }

    fn transcribe<'a>(&'a self, recording: &'a Recording) -> Transcribing<'a> {
        // All of the work is a comparison, so the future is already finished
        // when it is handed back. Boxing it is the price of the trait being
        // dyn-compatible, not a thread or a task.
        Box::pin(std::future::ready(self.look_up(recording)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> &'static Fixture {
        FIXTURES
            .iter()
            .find(|fixture| fixture.name == name)
            .unwrap_or_else(|| panic!("`{name}` is a fixture of this crate"))
    }

    #[tokio::test]
    async fn a_known_recording_transcribes_to_the_words_it_stands_for() {
        let fixture = fixture("payment-unit-1204.pcm");
        let recording = Recording::new(fixture_pcm(fixture)).expect("a fixture is a recording");
        let transcript = StubTranscriber
            .transcribe(&recording)
            .await
            .expect("the stub knows its own fixtures");
        assert_eq!(
            transcript.alternatives,
            vec!["what is the payment status of unit twelve oh four".to_owned()],
        );
        assert!(
            transcript.note.contains("No speech service was called"),
            "{}",
            transcript.note,
        );
    }

    #[tokio::test]
    async fn an_unknown_recording_is_refused_rather_than_guessed_at() {
        // Silence of exactly the same length as a fixture: the one input a
        // stub matching on length alone would answer with somebody else's
        // sentence.
        let silence = vec![0u8; fixture("payment-unit-1204.pcm").byte_len()];
        let recording = Recording::new(silence).expect("silence is still a recording");
        let error = StubTranscriber
            .transcribe(&recording)
            .await
            .expect_err("the stub has no transcript for this");
        let message = error.to_string();
        assert!(
            message.contains("nothing was guessed"),
            "the stub must say it has no transcript rather than invent one: {message}",
        );
    }

    #[test]
    fn every_fixture_is_a_distinct_recording_of_the_length_it_claims() {
        for fixture in &FIXTURES {
            let pcm = fixture_pcm(fixture);
            assert_eq!(pcm.len(), fixture.byte_len(), "{}", fixture.name);
            assert_eq!(pcm.len() % BYTES_PER_SAMPLE, 0, "{}", fixture.name);
            assert!(
                !fixture.heard.is_empty(),
                "a fixture with no transcript stands for nothing: {}",
                fixture.name,
            );
        }
        for (index, fixture) in FIXTURES.iter().enumerate() {
            for other in &FIXTURES[index + 1..] {
                assert_ne!(
                    fixture_pcm(fixture),
                    fixture_pcm(other),
                    "`{}` and `{}` are the same recording",
                    fixture.name,
                    other.name,
                );
            }
        }
    }

    #[test]
    fn the_generator_is_the_same_bytes_every_time_it_is_asked() {
        // The committed `.pcm` files are only trustworthy if this holds.
        for fixture in &FIXTURES {
            assert_eq!(
                fixture_pcm(fixture),
                fixture_pcm(fixture),
                "{}",
                fixture.name
            );
        }
    }

    #[test]
    fn a_recording_that_is_not_whole_samples_is_refused() {
        let error = Recording::new(vec![0u8; 3]).expect_err("three bytes is not 16-bit samples");
        assert!(matches!(error, TranscribeError::NotAligned { bytes: 3 }));
    }

    #[test]
    fn an_empty_recording_is_refused() {
        assert!(matches!(
            Recording::new(Vec::new()).expect_err("nothing was recorded"),
            TranscribeError::Empty,
        ));
    }

    #[test]
    fn a_recording_past_the_cap_is_refused_whole_rather_than_truncated() {
        let error =
            Recording::new(vec![0u8; MAX_RECORDING_BYTES + 2]).expect_err("it is past the cap");
        let message = error.to_string();
        assert!(
            message.contains("none of it was transcribed"),
            "an over-long recording must not be quietly cut down: {message}",
        );
    }
}
