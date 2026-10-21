//! The shared AWS configuration is never read by this crate's tests.
//!
//! Naming a profile that does not exist does not confine what is opened:
//! `aws_config` parses `~/.aws/config` and `~/.aws/credentials` whatever
//! profile is asked for, and simply finds no matching section. A test suite
//! that leans on an unused profile name is therefore still reading the
//! developer's real AWS configuration on every run.
//!
//! [`ask_aunova_simulator::aws::for_profile`] takes the profile files as an
//! argument so that this cannot happen by omission. This test is the proof:
//! it points `HOME` at an empty directory and clears the overrides, so the
//! default chain has nothing to find, and then asserts the configuration
//! comes out exactly as it does with those files present. If the shared files
//! were still being consulted, a machine with a `[default]` region would give
//! a different answer here from one without, and this would stop agreeing
//! with itself across machines.
//!
//! It is the only test in this binary on purpose. It mutates the process
//! environment, which is sound when nothing else is running in the process
//! and a race waiting to happen when something is.

#![cfg(feature = "live")]

use std::path::{Path, PathBuf};

use ask_aunova_simulator::aws::{DEFAULT_REGION, for_profile};
use ask_aunova_simulator::model::live::bedrock_config;
use aws_credential_types::provider::ProvideCredentials;
use aws_runtime::env_config::file::{EnvConfigFileKind, EnvConfigFiles};

/// A profile file that defines no profiles, committed next to this test.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-profiles.ini")
}

/// Both kinds pointed at the fixture, so neither default file is consulted.
fn only_the_fixture() -> EnvConfigFiles {
    EnvConfigFiles::builder()
        .with_file(EnvConfigFileKind::Config, fixture())
        .with_file(EnvConfigFileKind::Credentials, fixture())
        .build()
}

#[tokio::test]
async fn the_ambient_aws_configuration_is_ignored() {
    let empty_home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("empty-home");
    std::fs::create_dir_all(&empty_home).expect("a directory under this crate's target dir");

    // SAFETY: this is the only test in this binary, so no other thread is
    // reading the environment while it is written.
    //
    // `AWS_CONFIG_FILE` is pointed at a file that DOES define a region, and
    // deliberately not this project's fallback one. That is the experiment:
    // the SDK's default profile resolution would find it, so if the explicit
    // set passed below were ignored, that region would arrive here instead.
    // Emptying `HOME` as well leaves the planted file as the only AWS
    // configuration reachable by any route.
    unsafe {
        std::env::set_var("HOME", &empty_home);
        std::env::set_var(
            "AWS_CONFIG_FILE",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/leaky-profile.ini"),
        );
        std::env::remove_var("AWS_SHARED_CREDENTIALS_FILE");
        std::env::remove_var("AWS_PROFILE");
        std::env::remove_var("AWS_REGION");
        std::env::remove_var("AWS_DEFAULT_REGION");
    }

    // `default` on purpose: the one name that matches a section in the
    // planted file, so a leak has every chance to happen.
    let config = for_profile("default", only_the_fixture()).await;

    assert_eq!(
        config.region().map(|region| region.to_string()),
        Some(DEFAULT_REGION.to_owned()),
        "the region had to come from this crate's own fallback. Seeing eu-north-1 instead means \
         the profile files passed in were ignored and the ambient AWS configuration was read — \
         which on a developer's machine is their own ~/.aws/config",
    );
    assert!(
        config.http_client().is_some(),
        "and the configuration still carries this crate's own HTTP client",
    );

    // --- and now the environment, in the same test and so the same thread.
    //
    // Both halves mutate process-wide variables; two tests doing that in one
    // binary would race, so this is deliberately one test with two parts.
    // The SDK's default credentials chain asks the environment BEFORE it asks
    // the profile, so an exported key pair silently signs as a different
    // identity than the profile named on the command line. This crate pins
    // the credentials provider to the profile instead, and this is what says
    // so: with a key pair exported and a profile file that defines nothing,
    // resolving credentials must FAIL rather than hand back the env key.
    //
    // The values below are not a secret and are not a real key; they are a
    // shape the SDK would accept, so that a leak would be a success carrying
    // exactly these bytes.
    unsafe {
        std::env::set_var("AWS_ACCESS_KEY_ID", "NOTAREALKEYIDFORATEST");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "not-a-real-secret-for-a-test");
        std::env::remove_var("AWS_SESSION_TOKEN");
    }

    let config = for_profile("default", only_the_fixture()).await;
    let provider = config
        .credentials_provider()
        .expect("a credentials provider is configured");

    if let Ok(credentials) = provider.provide_credentials().await {
        panic!(
            "credentials resolved from somewhere, and with an empty profile file the only \
             somewhere left is the environment: access key id {}",
            credentials.access_key_id()
        );
    }

    // --- and the third door: Bedrock's own bearer token.
    //
    // `aws-sdk-bedrockruntime` reads AWS_BEARER_TOKEN_BEDROCK and switches
    // the client to bearer auth when it is set, which signs as whoever owns
    // that token rather than the named profile. Pinning the credentials
    // provider does not close this one: it is an auth-scheme decision, not a
    // credentials one, so it needs its own bolt and its own assertion.
    unsafe {
        std::env::set_var(
            "AWS_BEARER_TOKEN_BEDROCK",
            "not-a-real-bedrock-bearer-token",
        );
    }

    // The observable is the auth scheme preference the config resolves to.
    // With the token exported, the stock config's preference list becomes
    // `httpBearerAuth`; the guarded one stays `sigv4`, which is the scheme
    // that signs with the profile's credentials.
    //
    // The preference LIST is the right thing to assert, not the absence of
    // the string `httpBearerAuth` anywhere in the config: the scheme stays
    // registered either way, as one the client knows how to speak. Being
    // registered is not being chosen, and a test that confused the two
    // failed against a correct fix.
    let ours = format!("{:?}", bedrock_config(&config));
    let unguarded = format!("{:?}", aws_sdk_bedrockruntime::Config::from(&config));

    // Control first: without this, the assertion after it could pass simply
    // because the variable never took effect, and a test that cannot fail is
    // worse than no test.
    assert!(
        unguarded.contains(
            "AuthSchemePreference { preference_list: [AuthSchemeId { scheme_id: \
             \"httpBearerAuth\" }] }"
        ),
        "the exported bearer token did not reach even the unguarded config, so this test is \
         not exercising what it claims to: {unguarded}",
    );
    assert!(
        ours.contains(
            "AuthSchemePreference { preference_list: [AuthSchemeId { scheme_id: \"sigv4\" }] }"
        ),
        "the Bedrock client did not stay on SigV4 with a bearer token exported, so it would \
         sign as the owner of that token instead of the named profile: {ours}",
    );
}
