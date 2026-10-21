//! The one address this client is allowed to talk to.
//!
//! Every error message in this crate names an endpoint, and an endpoint is
//! only safe to print because [`Endpoint::parse`] refused the parts that are
//! not: a URL carrying userinfo, or a query string, can put a credential in a
//! log line. Refusing them at parse time is what makes `{endpoint}` safe
//! everywhere else, and is why no transport error is ever rendered directly —
//! `reqwest::Error`'s own `Display` embeds the request URL it was given.
//!
//! It also refuses anything but loopback. This binary holds the two demo
//! bearer tokens; sending them to a host outside this machine is not a
//! configuration mistake worth tolerating.

use std::fmt;

/// A validated `http://<loopback>[:port]/<path>` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    url: String,
}

/// Why an address was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EndpointError {
    /// Nothing that looks like `scheme://host/path`.
    #[error("`{raw}` is not an absolute URL: expected `http://host:port/path`")]
    NotAbsolute { raw: String },
    /// Only plain HTTP, because no TLS backend is compiled in.
    #[error("`{scheme}` is not a scheme this client speaks; it talks plain `http` to loopback")]
    Scheme { scheme: String },
    /// `user:password@host` would put a credential in every log line.
    #[error("an endpoint may not carry userinfo before the host")]
    Userinfo,
    /// A query or a fragment can carry a key, and `/mcp` needs neither.
    #[error("an endpoint may not carry a `{part}`; the MCP endpoint is a path")]
    Decoration { part: &'static str },
    /// No host at all.
    #[error("`{raw}` names no host")]
    NoHost { raw: String },
    /// The whole policy: this client never leaves the machine.
    #[error(
        "`{host}` is not loopback; this client speaks only to 127.0.0.0/8, ::1 or localhost, \
         because it carries bearer tokens"
    )]
    NotLoopback { host: String },
}

impl Endpoint {
    /// Validate one address, or say exactly what is wrong with it.
    pub fn parse(raw: &str) -> Result<Self, EndpointError> {
        let raw = raw.trim();
        let (scheme, rest) = raw
            .split_once("://")
            .ok_or_else(|| EndpointError::NotAbsolute {
                raw: raw.to_owned(),
            })?;
        if !scheme.eq_ignore_ascii_case("http") {
            return Err(EndpointError::Scheme {
                scheme: scheme.to_owned(),
            });
        }
        if rest.contains('#') {
            return Err(EndpointError::Decoration { part: "fragment" });
        }
        if rest.contains('?') {
            return Err(EndpointError::Decoration { part: "query" });
        }

        let (authority, path) = match rest.find('/') {
            Some(cut) => (&rest[..cut], &rest[cut..]),
            None => (rest, "/"),
        };
        if authority.contains('@') {
            return Err(EndpointError::Userinfo);
        }
        let host = host_of(authority);
        if host.is_empty() {
            return Err(EndpointError::NoHost {
                raw: raw.to_owned(),
            });
        }
        if !is_loopback(host) {
            return Err(EndpointError::NotLoopback {
                host: host.to_owned(),
            });
        }

        Ok(Self {
            url: format!("http://{authority}{path}"),
        })
    }

    /// The address to connect to, which is also the address it is safe to
    /// print: [`Endpoint::parse`] refused everything that would not be.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.url)
    }
}

/// The host inside an authority, with an IPv6 literal's brackets kept.
///
/// Splitting on the last colon would cut `[::1]` in half, so a bracketed
/// literal is taken whole and only what follows it can be a port.
fn host_of(authority: &str) -> &str {
    if let Some(end) = authority.find(']') {
        return &authority[..=end];
    }
    match authority.rsplit_once(':') {
        Some((host, _port)) => host,
        None => authority,
    }
}

/// Whether a host names this machine and only this machine.
fn is_loopback(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match bare.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_loopback_endpoint_whole() {
        let endpoint = Endpoint::parse("http://127.0.0.1:8731/mcp").expect("loopback is allowed");
        assert_eq!(endpoint.url(), "http://127.0.0.1:8731/mcp");
    }

    #[test]
    fn accepts_every_loopback_spelling() {
        for raw in [
            "http://localhost:8731/mcp",
            "http://127.0.0.2:8731/mcp",
            "http://[::1]:8731/mcp",
        ] {
            assert!(Endpoint::parse(raw).is_ok(), "{raw} should be loopback");
        }
    }

    #[test]
    fn defaults_a_missing_path_to_root() {
        let endpoint = Endpoint::parse("http://127.0.0.1:8731").expect("a bare host is allowed");
        assert_eq!(endpoint.url(), "http://127.0.0.1:8731/");
    }

    #[test]
    fn refuses_a_host_outside_this_machine() {
        assert_eq!(
            Endpoint::parse("http://example.invalid/mcp"),
            Err(EndpointError::NotLoopback {
                host: "example.invalid".to_owned()
            }),
        );
    }

    #[test]
    fn refuses_userinfo_before_the_host() {
        assert_eq!(
            Endpoint::parse("http://user:secret@127.0.0.1:8731/mcp"),
            Err(EndpointError::Userinfo),
        );
    }

    #[test]
    fn refuses_a_query_or_a_fragment() {
        assert_eq!(
            Endpoint::parse("http://127.0.0.1:8731/mcp?apikey=hunter2"),
            Err(EndpointError::Decoration { part: "query" }),
        );
        assert_eq!(
            Endpoint::parse("http://127.0.0.1:8731/mcp#top"),
            Err(EndpointError::Decoration { part: "fragment" }),
        );
    }

    #[test]
    fn refuses_a_scheme_this_build_cannot_speak() {
        assert_eq!(
            Endpoint::parse("https://127.0.0.1:8731/mcp"),
            Err(EndpointError::Scheme {
                scheme: "https".to_owned()
            }),
        );
    }

    #[test]
    fn refuses_something_that_is_not_a_url() {
        assert_eq!(
            Endpoint::parse("127.0.0.1:8731"),
            Err(EndpointError::NotAbsolute {
                raw: "127.0.0.1:8731".to_owned()
            }),
        );
    }
}
