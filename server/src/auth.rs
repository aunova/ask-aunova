//! Bearer token to role, resolved server-side.
//!
//! The client never sends a role. It sends a bearer token, and this module maps
//! that token to a [`Role`] *before* the MCP transport sees the request. A
//! missing, malformed or unknown token is answered with `401` and a body that
//! says nothing about which tokens exist.
//!
//! Nothing else about the caller is derived here, and nothing is stored. This
//! server keeps no per-caller state for the token to key — `whats_changed`
//! hands the caller a cursor instead — so the token is read, compared, mapped
//! to a role and dropped.
//!
//! The two demo identities are deliberately low-entropy, obviously fake strings
//! belonging to a synthetic dataset. They are not secrets and are not treated as
//! secrets; they are overridable by environment variable so that anyone running
//! this repo can swap them.

use std::fmt;

use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Demo bearer token for the buyer identity.
pub const DEFAULT_BUYER_TOKEN: &str = "demo-token-buyer-1204";
/// Demo bearer token for the developer identity.
pub const DEFAULT_DEVELOPER_TOKEN: &str = "demo-token-developer";
/// Environment variable overriding [`DEFAULT_BUYER_TOKEN`].
pub const BUYER_TOKEN_ENV: &str = "ASK_AUNOVA_BUYER_TOKEN";
/// Environment variable overriding [`DEFAULT_DEVELOPER_TOKEN`].
pub const DEVELOPER_TOKEN_ENV: &str = "ASK_AUNOVA_DEVELOPER_TOKEN";
/// The unit in the synthetic project that the demo buyer bought.
pub const DEMO_BUYER_UNIT: &str = "1204";

/// Body returned with every `401`. It is identical for an absent, a malformed
/// and an unknown token, so it cannot be used to probe which tokens exist.
const UNAUTHORIZED_BODY: &str = concat!(
    r#"{"error":"unauthorized","message":"#,
    r#""This server resolves your role from a bearer token. "#,
    r#"Send an Authorization: Bearer <token> header."}"#,
);

/// What a caller is allowed to hear, resolved from their token.
///
/// A buyer is bound to the single unit they bought; the developer sees the whole
/// project. The tools read this instead of trusting anything in the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// A buyer, scoped to one unit of the project.
    Buyer {
        /// Unit number as printed in the sale agreement.
        unit: String,
    },
    /// The project developer: every unit, plus internal evidence.
    Developer,
}

impl fmt::Display for Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Buyer { unit } => write!(formatter, "buyer:{unit}"),
            Role::Developer => formatter.write_str("developer"),
        }
    }
}

/// Why a request was refused. Logged, never sent to the caller: the response
/// body is the same for all three so it leaks nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthRejection {
    /// No `Authorization` header at all.
    MissingHeader,
    /// An `Authorization` header that is not a non-empty `Bearer <token>`.
    MalformedHeader,
    /// A well-formed bearer token that is not one of the demo identities.
    UnknownToken,
}

impl fmt::Display for AuthRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            AuthRejection::MissingHeader => "request carried no Authorization header",
            AuthRejection::MalformedHeader => {
                "Authorization header is not a non-empty `Bearer <token>` value"
            }
            AuthRejection::UnknownToken => "bearer token is not one of the demo identities",
        })
    }
}

/// The tokens this server accepts and the role each one grants.
#[derive(Clone, Debug)]
pub struct TokenRegistry {
    buyer_token: String,
    developer_token: String,
    buyer_unit: String,
}

impl TokenRegistry {
    /// Build a registry from explicit values. Callers outside this module go
    /// through [`TokenRegistry::from_lookup`] or [`TokenRegistry::from_env`].
    fn new(
        buyer_token: impl Into<String>,
        developer_token: impl Into<String>,
        buyer_unit: impl Into<String>,
    ) -> Self {
        Self {
            buyer_token: buyer_token.into(),
            developer_token: developer_token.into(),
            buyer_unit: buyer_unit.into(),
        }
    }

    /// Build a registry from a name-to-value lookup, falling back to the demo
    /// tokens. [`TokenRegistry::from_env`] passes the process environment; the
    /// tests pass a fake, which keeps them free of process-wide mutation.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        Self::new(
            lookup(BUYER_TOKEN_ENV).unwrap_or_else(|| DEFAULT_BUYER_TOKEN.to_owned()),
            lookup(DEVELOPER_TOKEN_ENV).unwrap_or_else(|| DEFAULT_DEVELOPER_TOKEN.to_owned()),
            DEMO_BUYER_UNIT,
        )
    }

    /// Build a registry from the environment, falling back to the demo tokens.
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Resolve an `Authorization` header value to a role.
    ///
    /// Both candidate tokens are compared on every call, and each comparison
    /// runs to the end of the token, so neither which identity matched nor how
    /// many leading bytes were right is readable from the time taken.
    pub fn resolve(&self, header: Option<&HeaderValue>) -> Result<Role, AuthRejection> {
        let header = header.ok_or(AuthRejection::MissingHeader)?;
        let value = header
            .to_str()
            .map_err(|_not_visible_ascii| AuthRejection::MalformedHeader)?;
        let token = bearer_token(value).ok_or(AuthRejection::MalformedHeader)?;

        let is_buyer = constant_time_eq(token.as_bytes(), self.buyer_token.as_bytes());
        let is_developer = constant_time_eq(token.as_bytes(), self.developer_token.as_bytes());

        match (is_buyer, is_developer) {
            (_, true) => Ok(Role::Developer),
            (true, false) => Ok(Role::Buyer {
                unit: self.buyer_unit.clone(),
            }),
            (false, false) => Err(AuthRejection::UnknownToken),
        }
    }
}

/// Extract the token from a `Bearer <token>` header value.
fn bearer_token(value: &str) -> Option<&str> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// Compare two byte strings without exiting early on the first differing byte.
///
/// Lengths are compared up front and are not treated as secret: the demo tokens
/// are fixed, published strings, so the only thing a length check can reveal is
/// the length of a token the caller already holds. `black_box` stops the
/// optimiser from reintroducing the early exit over the body.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left_byte, right_byte) in left.iter().zip(right.iter()) {
        difference |= left_byte ^ right_byte;
    }
    std::hint::black_box(difference) == 0
}

/// Middleware: resolve the caller's role and attach it to the request, or
/// answer `401`.
///
/// The resolved [`Role`] goes into the HTTP request extensions. The Streamable
/// HTTP transport then hands those extensions to the MCP request context,
/// which is where the tools read it from. That is the whole path from token to
/// tool, and it has no branch that lets a client name its own role.
pub async fn require_bearer_token(
    tokens: &TokenRegistry,
    mut request: Request,
    next: Next,
) -> Response {
    match tokens.resolve(request.headers().get(header::AUTHORIZATION)) {
        Ok(role) => {
            // The role is logged; the token never is.
            tracing::debug!(%role, path = %request.uri().path(), "authorized MCP request");
            request.extensions_mut().insert(role);
            next.run(request).await
        }
        Err(rejection) => {
            // The rejection reason is logged; the token value never is.
            tracing::warn!(
                reason = %rejection,
                path = %request.uri().path(),
                "refused an MCP request: no role could be resolved",
            );
            unauthorized()
        }
    }
}

/// Build the `401` response, with the `WWW-Authenticate` challenge the MCP
/// authorization spec expects.
fn unauthorized() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, UNAUTHORIZED_BODY).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> TokenRegistry {
        TokenRegistry::from_lookup(|_| None)
    }

    fn header(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).expect("test header value is visible ASCII")
    }

    fn role_for(registry: &TokenRegistry, value: &str) -> Result<Role, AuthRejection> {
        registry.resolve(Some(&header(value)))
    }

    #[test]
    fn buyer_token_resolves_to_a_unit_scoped_role() {
        let resolved = role_for(&registry(), "Bearer demo-token-buyer-1204");
        assert_eq!(
            resolved,
            Ok(Role::Buyer {
                unit: "1204".to_owned()
            })
        );
        assert_eq!(
            resolved.map(|role| role.to_string()),
            Ok("buyer:1204".to_owned())
        );
    }

    #[test]
    fn developer_token_resolves_to_the_developer_role() {
        let resolved = role_for(&registry(), "Bearer demo-token-developer");
        assert_eq!(resolved, Ok(Role::Developer));
        assert_eq!(
            resolved.map(|role| role.to_string()),
            Ok("developer".to_owned())
        );
    }

    #[test]
    fn an_absent_authorization_header_is_rejected() {
        assert_eq!(registry().resolve(None), Err(AuthRejection::MissingHeader));
    }

    #[test]
    fn a_malformed_authorization_header_is_rejected() {
        for value in [
            "demo-token-developer",
            "Basic demo-token-developer",
            "Bearer ",
            "Bearer    ",
        ] {
            assert_eq!(
                role_for(&registry(), value),
                Err(AuthRejection::MalformedHeader),
                "`{value}` should not authenticate anyone",
            );
        }
    }

    #[test]
    fn an_unknown_token_is_rejected() {
        for value in [
            "Bearer demo-token-buyer-0802",
            "Bearer DEMO-TOKEN-DEVELOPER",
            "Bearer demo-token-developerr",
            "Bearer demo-token-develope",
        ] {
            assert_eq!(
                role_for(&registry(), value),
                Err(AuthRejection::UnknownToken),
                "`{value}` is not a demo identity and must not resolve to a role",
            );
        }
    }

    #[test]
    fn surrounding_whitespace_in_the_header_is_tolerated() {
        assert_eq!(
            role_for(&registry(), "Bearer  demo-token-buyer-1204 "),
            Ok(Role::Buyer {
                unit: "1204".to_owned()
            }),
        );
    }

    #[test]
    fn the_bearer_scheme_is_case_insensitive_but_the_token_is_not() {
        assert_eq!(
            role_for(&registry(), "bearer demo-token-developer"),
            Ok(Role::Developer),
        );
        assert_eq!(
            role_for(&registry(), "Bearer Demo-Token-Developer"),
            Err(AuthRejection::UnknownToken),
        );
    }

    #[test]
    fn environment_values_override_the_demo_tokens() {
        let tokens = TokenRegistry::from_lookup(|name| match name {
            BUYER_TOKEN_ENV => Some("judge-buyer".to_owned()),
            DEVELOPER_TOKEN_ENV => Some("judge-developer".to_owned()),
            _ => None,
        });
        assert_eq!(
            role_for(&tokens, "Bearer judge-buyer"),
            Ok(Role::Buyer {
                unit: "1204".to_owned()
            }),
        );
        assert_eq!(
            role_for(&tokens, "Bearer judge-developer"),
            Ok(Role::Developer),
        );
        assert_eq!(
            role_for(&tokens, "Bearer demo-token-developer"),
            Err(AuthRejection::UnknownToken),
            "an overridden server must stop accepting the default token",
        );
    }

    #[test]
    fn constant_time_eq_matches_ordinary_equality() {
        for (left, right) in [
            (&b""[..], &b""[..]),
            (b"abc", b"abc"),
            (b"abc", b"abd"),
            (b"abc", b"abcd"),
            (b"", b"a"),
        ] {
            assert_eq!(
                constant_time_eq(left, right),
                left == right,
                "{left:?} vs {right:?}",
            );
        }
    }
}
