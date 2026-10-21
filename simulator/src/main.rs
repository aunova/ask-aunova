#![forbid(unsafe_code)]
//! Run the voice-assistant simulator on loopback.
//!
//! Page: `ASK_AUNOVA_SIMULATOR_PORT` (default 8732), bound to 127.0.0.1 and
//! nowhere else. MCP server: `ASK_AUNOVA_MCP_URL` (default
//! `http://127.0.0.1:8731/mcp`), which must itself be loopback. Bearer tokens:
//! `ASK_AUNOVA_BUYER_TOKEN` and `ASK_AUNOVA_DEVELOPER_TOKEN`, the same two
//! variables the server reads, so a server started with non-default tokens
//! needs no second configuration here.
//!
//! By default this runs on the stub model, the stub transcriber and the
//! silent voice, and the page says so: nothing in it calls a hosted service.
//! Passing `--live` to a binary built with the `live` feature swaps in Amazon
//! Nova Pro on Bedrock and Amazon Polly; `--live-voice` takes the voice
//! alone; Amazon Transcribe needs `--live-transcribe` on top. Credentials
//! come from the shared profile named by `AWS_PROFILE` and from nothing
//! else — see [`ask_aunova_simulator::aws`], which pins the credentials
//! provider to that profile so an exported key pair cannot quietly replace
//! it. A compile-time switch and a runtime one are both needed, and neither
//! is the default.

use std::{process::ExitCode, sync::Arc};

use ask_aunova_simulator::{
    PORT_ENV, StartupError, endpoint_from_env, listen_addr,
    model::Model,
    port_from_env,
    stub::StubModel,
    transcribe::{StubTranscriber, Transcriber},
    voice::{SilentVoice, Voice},
    web,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error}");
            let mut cause = std::error::Error::source(&error);
            while let Some(reason) = cause {
                tracing::error!("  caused by: {reason}");
                cause = reason.source();
            }
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), StartupError> {
    let endpoint = endpoint_from_env()?;
    let addr = listen_addr(port_from_env()?);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| StartupError::Bind { addr, source })?;
    // Re-read the address from the socket: started with port 0, the bound
    // port is the only one worth printing.
    let bound = listener
        .local_addr()
        .map_err(|source| StartupError::Bind { addr, source })?;

    let switches = Switches::from_args(std::env::args().skip(1));
    if switches.transcribe && !switches.live {
        return Err(StartupError::TranscribeWithoutLive);
    }
    let (model, voice, transcriber) = if switches.live || switches.voice {
        live_parts(&switches).await?
    } else {
        (
            Box::new(StubModel) as Box<dyn Model>,
            Box::new(SilentVoice) as Box<dyn Voice>,
            Box::new(StubTranscriber) as Box<dyn Transcriber>,
        )
    };
    tracing::info!(
        "Ask Aunova simulator listening on http://{bound} (loopback only); it is an MCP client \
         of {endpoint}, answering with the {} and speaking with the {} voice. Push-to-talk is \
         transcribed by the {}. Set {PORT_ENV} to move this page.",
        model.name(),
        voice.name(),
        transcriber.name(),
    );
    let simulator = Arc::new(web::Simulator::new(endpoint, model, voice, transcriber));

    axum::serve(listener, web::app(simulator))
        .await
        .map_err(|source| StartupError::Serve {
            addr: bound,
            source,
        })
}

/// What the command line asked for.
///
/// Parsed from an iterator rather than read from the environment so that the
/// rules — and particularly which hosted service each switch turns on — are
/// testable without starting a process.
///
/// Three switches rather than one because the three seams send different
/// things to different places, and agreeing to one is not agreeing to the
/// others. The model sends the question and the tool results; the voice
/// sends the finished answer; the transcriber sends a recording of somebody
/// speaking.
#[derive(Debug, Default, PartialEq, Eq)]
struct Switches {
    /// Use the hosted model. Implies the hosted voice.
    live: bool,
    /// Use the hosted voice, with or without the hosted model.
    voice: bool,
    /// Also use the hosted transcriber.
    transcribe: bool,
}

impl Switches {
    /// Read the switches out of the arguments, ignoring anything else.
    fn from_args(args: impl Iterator<Item = String>) -> Self {
        let mut switches = Self::default();
        for arg in args {
            match arg.as_str() {
                "--live" => switches.live = true,
                "--live-voice" => switches.voice = true,
                "--live-transcribe" => switches.transcribe = true,
                _ => {}
            }
        }
        switches
    }
}

/// The hosted parts the switches asked for, and stubs for the rest.
///
/// `--live` is the hosted model, and it implies the hosted voice: a model
/// that composes an answer nobody can hear is not the thing being
/// demonstrated. `--live-voice` is the voice on its own, which is what makes
/// the demonstration possible when the model is unavailable — the scripted
/// stub answers and the hosted voice speaks it.
///
/// The transcriber stays off in both. It is the only one that sends a
/// recording of somebody's voice rather than text, and that is a different
/// consent.
#[cfg(feature = "live")]
async fn live_parts(
    switches: &Switches,
) -> Result<(Box<dyn Model>, Box<dyn Voice>, Box<dyn Transcriber>), StartupError> {
    use ask_aunova_simulator::{
        model::live::BedrockModel, transcribe::live::StreamingTranscriber, voice::live::PollyVoice,
    };
    use aws_sdk_transcribestreaming::types::LanguageCode;

    let configured = |detail: String| StartupError::LiveNotConfigured { detail };

    let model: Box<dyn Model> = if switches.live {
        Box::new(
            BedrockModel::new()
                .await
                .map_err(|error| configured(error.to_string()))?,
        )
    } else {
        Box::new(StubModel)
    };

    let voice = PollyVoice::new()
        .await
        .map_err(|error| configured(error.to_string()))?;

    let transcriber: Box<dyn Transcriber> = if switches.transcribe {
        Box::new(
            StreamingTranscriber::new(LanguageCode::EnGb)
                .await
                .map_err(|error| configured(error.to_string()))?,
        )
    } else {
        Box::new(StubTranscriber)
    };

    Ok((model, Box::new(voice), transcriber))
}

/// Without the `live` feature there is nothing to swap in, and asking for it
/// is a refusal rather than a silent fall back to the stubs.
#[cfg(not(feature = "live"))]
async fn live_parts(
    _switches: &Switches,
) -> Result<(Box<dyn Model>, Box<dyn Voice>, Box<dyn Transcriber>), StartupError> {
    Err(StartupError::LiveNotCompiledIn)
}

/// Start logging.
///
/// With the `live` feature the AWS SDK is in this process, and `aws_config`
/// logs the resolved access key id at INFO when it builds a credentials
/// provider. That is a credential in a terminal that may be recorded, so the
/// SDK's own targets are held at WARN and only this crate speaks at INFO.
///
/// `RUST_LOG` is deliberately not read. Honouring it would hand the leak
/// straight back to anyone who exported it, and the claim in
/// [`ask_aunova_simulator::aws`] that nothing here can log a credential has
/// to be true of every run, not of the runs with a tidy environment.
#[cfg(feature = "live")]
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "info,aws_config=warn,aws_smithy_runtime=warn,aws_smithy_runtime_api=warn,\
             aws_sdk_bedrockruntime=warn,aws_sdk_polly=warn,aws_sdk_transcribestreaming=warn",
        ))
        .init();
}

/// Without the `live` feature no AWS SDK is linked, so there is no SDK target
/// to quieten and nothing in the process that holds a credential.
#[cfg(not(feature = "live"))]
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn switches(args: &[&str]) -> Switches {
        Switches::from_args(args.iter().map(|arg| (*arg).to_string()))
    }

    #[test]
    fn nothing_hosted_is_the_default() {
        assert_eq!(switches(&[]), Switches::default());
        assert!(!switches(&[]).live);
        assert!(!switches(&[]).voice);
        assert!(!switches(&[]).transcribe);
    }

    #[test]
    fn the_voice_can_be_hosted_without_the_model() {
        // The demonstration the owner asked for is a spoken answer. When the
        // model is unavailable the scripted stub can answer and the hosted
        // voice can still speak it, so the voice has to be reachable on its
        // own rather than only as part of `--live`.
        let asked = switches(&["--live-voice"]);
        assert!(asked.voice);
        assert!(!asked.live, "--live-voice must not switch on the model");
        assert!(
            !asked.transcribe,
            "and it must not switch on the transcriber either",
        );
    }

    #[test]
    fn live_alone_does_not_switch_on_the_transcriber() {
        // The model and the voice send text; the transcriber sends a
        // recording of someone's voice. `--live` is not consent to the
        // second, so it must not turn it on.
        let asked = switches(&["--live"]);
        assert!(asked.live);
        assert!(
            !asked.transcribe,
            "--live must leave the hosted transcriber off",
        );
    }

    #[test]
    fn the_transcriber_needs_both_switches() {
        let asked = switches(&["--live", "--live-transcribe"]);
        assert!(asked.live && asked.transcribe);
    }

    #[test]
    fn asking_only_for_the_transcriber_is_not_silently_ignored() {
        // Reaching `run` with this would quietly serve stubs, so the switch
        // is recorded here and refused there rather than dropped.
        let asked = switches(&["--live-transcribe"]);
        assert!(!asked.live);
        assert!(asked.transcribe);
    }

    #[test]
    fn an_unknown_argument_is_not_a_switch() {
        assert_eq!(
            switches(&["--livery", "live", "-live"]),
            Switches::default()
        );
    }
}
