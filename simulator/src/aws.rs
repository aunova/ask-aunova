//! The one place this crate builds an AWS configuration.
//!
//! **Credentials are never read from this crate.** They come from the shared
//! profile named by `AWS_PROFILE`, and from nothing else. This crate holds no
//! access key, no secret and no session token, and has no code path that
//! could log one: the only thing it names is the profile.
//!
//! "And from nothing else" is load bearing. The SDK's *default* credentials
//! chain asks the environment before it asks the profile, so an exported
//! `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` would quietly win over the
//! profile named on the command line and sign as a different identity.
//! [`for_profile`] therefore pins the credentials provider to the profile
//! rather than taking that chain.
//!
//! **The HTTP client is chosen explicitly, for one reason.** Taking the SDK's
//! `default-https-client` would pull `rustls-aws-lc`, which is 11,607
//! `unsafe` across `aws-lc-rs` and `aws-lc-sys` plus a cmake-driven build of
//! a vendored C library. Naming `aws-smithy-http-client` with `rustls-ring`
//! is the same rustls and the same TLS over 230 `unsafe` in ring.
//!
//! This is a smaller claim than "no C": ring compiles C and assembly of its
//! own and needs a C compiler to build. What it does not need is cmake or a
//! vendored copy of a whole TLS library. `cargo tree` is the check for
//! `aws-lc`, and it is in the report.
//!
//! **Nothing here consults the instance metadata service.** The region comes
//! from the environment, then the profile, then a constant — three providers
//! that read variables and files and nothing else. That is not a performance
//! choice. A provider built here, outside the loader, does not receive the
//! loader's HTTP client, and the metadata leg of the SDK's default region
//! chain reacts to that by constructing a request with no client and
//! panicking before any call is made. Dropping the leg removes the dependency
//! rather than papering over it, and on a machine that is not an EC2 instance
//! it could only ever have timed out.
//!
//! **The behaviour version is pinned in code** rather than taken from
//! `behavior-version-latest`, so upgrading the SDK cannot change how a request
//! is made without somebody editing this line.

use aws_config::{
    BehaviorVersion, Region, SdkConfig, environment::region::EnvironmentVariableRegionProvider,
    meta::region::RegionProviderChain, profile::credentials::ProfileFileCredentialsProvider,
    profile::region::ProfileFileRegionProvider, provider_config::ProviderConfig,
};
use aws_runtime::env_config::file::EnvConfigFiles;
use aws_smithy_http_client::{Builder as HttpBuilder, tls};

/// The region this demo's IAM policy allows. The policy denies every other
/// region, so a profile that names one would fail at the call rather than
/// here; this is the fallback when nothing names a region at all.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Environment variable naming the shared profile to use.
pub const PROFILE_ENV: &str = "AWS_PROFILE";

/// Why a live client could not be built.
#[derive(Debug, thiserror::Error)]
pub enum AwsError {
    /// No profile was named, so there is nothing to resolve credentials from.
    #[error(
        "set {PROFILE_ENV} to the AWS profile this demo should use; this build reads credentials \
         only from the shared profile chain and holds none of its own"
    )]
    NoProfile,
}

/// Build the SDK configuration every live client in this crate shares.
///
/// Fails only when no profile is named. A profile that exists but cannot
/// produce credentials fails later, at the call, where the SDK can say which
/// provider refused — this function deliberately does not probe, because
/// probing would be a live call made at startup.
pub async fn config() -> Result<SdkConfig, AwsError> {
    let profile = std::env::var(PROFILE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(AwsError::NoProfile)?;

    Ok(for_profile(&profile, EnvConfigFiles::default()).await)
}

/// The configuration for one named profile, from one named set of files.
///
/// `files` is a parameter rather than a default because naming a profile
/// does **not** confine what is read: both the loader and the region
/// provider parse the shared configuration — `~/.aws/config` and
/// `~/.aws/credentials` — whatever profile is asked for, and simply find no
/// section matching the name. A test that passes a name nobody has therefore
/// still reads the machine's real AWS files on every run, which is a file
/// nothing in a test suite has any business opening.
///
/// Passing the set in makes that visible at every call site: [`config`] uses
/// the real chain, which is the whole point of it, and a test passes a
/// fixture that contains nothing.
pub async fn for_profile(profile: &str, files: EnvConfigFiles) -> SdkConfig {
    let https = HttpBuilder::new()
        .tls_provider(tls::Provider::Rustls(
            tls::rustls_provider::CryptoMode::Ring,
        ))
        .build_https();

    // Every leg reads a variable or a file. None of them makes a request, so
    // none of them needs the HTTP client that this chain, built outside the
    // loader, could not give them anyway.
    let region = RegionProviderChain::first_try(EnvironmentVariableRegionProvider::new())
        .or_else(
            ProfileFileRegionProvider::builder()
                .profile_name(profile)
                .profile_files(files.clone())
                .build(),
        )
        .or_else(Region::new(DEFAULT_REGION));

    // Pinned to the profile, NOT the SDK's default credentials chain.
    //
    // That chain asks the environment before it asks the profile, so an
    // exported `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` silently win
    // over the profile this binary was told to use — quietly signing with a
    // different identity than the one named on the command line, which is
    // the sort of thing nobody notices until it matters. Everything this
    // crate says about reading credentials only from the named profile is
    // true because of this line.
    //
    // The HTTP client goes in because this provider is not always offline:
    // an SSO profile, or one with `role_arn`, has to call out to resolve,
    // and a provider built without a client panics when it tries.
    let credentials = ProfileFileCredentialsProvider::builder()
        .profile_name(profile)
        .profile_files(files.clone())
        .configure(&ProviderConfig::without_region().with_http_client(https.clone()))
        .build();

    aws_config::defaults(BehaviorVersion::v2026_01_12())
        .profile_name(profile)
        .profile_files(files)
        .credentials_provider(credentials)
        .region(region)
        .http_client(https)
        .load()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_runtime::env_config::file::EnvConfigFileKind;

    #[tokio::test]
    async fn an_unnamed_profile_is_refused_before_anything_is_built() {
        // Nothing here reaches the network: the refusal happens on the
        // environment, before a client exists. Guarding the variable rather
        // than setting it keeps this test honest on a machine that has one.
        if std::env::var(PROFILE_ENV).is_ok() {
            return;
        }
        let error = config().await.expect_err("an unnamed profile must refuse");
        assert!(
            error.to_string().contains(PROFILE_ENV),
            "the refusal must name the variable to set: {error}",
        );
    }

    #[tokio::test]
    async fn the_loader_resolves_a_region_without_building_a_clientless_request() {
        // The regression this exists for: the region chain used to be the
        // SDK's default one, built here rather than by the loader, so its
        // instance-metadata leg held no HTTP client. Resolving it constructed
        // a request with no client and panicked — at startup, before any call,
        // which no amount of credential checking would have found.
        //
        // Driving the real loader to completion is the test. The fixture is
        // a profile file with no profiles in it, so nothing here opens the
        // machine's own AWS configuration: naming a profile that does not
        // exist would not have been enough, because the shared files are read
        // whatever name is asked for.
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-profiles.ini");
        let files = EnvConfigFiles::builder()
            .with_file(EnvConfigFileKind::Config, &fixture)
            .with_file(EnvConfigFileKind::Credentials, &fixture)
            .build();
        let config = for_profile("ask-aunova-a-profile-that-does-not-exist", files).await;

        assert!(
            config.region().is_some(),
            "a region must always resolve, so that a call fails at the service rather than here",
        );
        assert!(
            config.http_client().is_some(),
            "the configuration must carry this crate's own HTTP client, not the SDK's default",
        );
    }
}
