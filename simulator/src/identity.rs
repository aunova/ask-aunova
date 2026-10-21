//! Which demo identity the MCP client authenticates as.
//!
//! This module is the entire role switch, and it is deliberately the only
//! thing a switch can reach. An [`Identity`] can do exactly one thing —
//! produce a bearer token — and that token is consumed once, when the
//! transport is built. Nothing downstream of that is given the identity: the
//! tool-use loop, the model and the answer composer have no parameter to
//! branch on, so there is no client-side filtering to go wrong and no second
//! code path to drift. The server resolves the role from the token and decides
//! what may be answered; this client cannot influence it and does not try.
//!
//! The two tokens are the public demo identities shipped with the synthetic
//! dataset — they are printed in the repository's own skill file — so they are
//! demonstration credentials, not secrets. They are still never logged and
//! never put in an error message, because the habit is the point.

use serde::{Deserialize, Serialize};

/// Environment variable overriding the buyer token.
pub const BUYER_TOKEN_ENV: &str = "ASK_AUNOVA_BUYER_TOKEN";
/// Environment variable overriding the developer token.
pub const DEVELOPER_TOKEN_ENV: &str = "ASK_AUNOVA_DEVELOPER_TOKEN";

/// The buyer demo token when [`BUYER_TOKEN_ENV`] is unset.
const DEFAULT_BUYER_TOKEN: &str = "demo-token-buyer-1204";
/// The developer demo token when [`DEVELOPER_TOKEN_ENV`] is unset.
const DEFAULT_DEVELOPER_TOKEN: &str = "demo-token-developer";

/// One of the two demo identities the page can switch between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Identity {
    /// The buyer of one unit.
    Buyer,
    /// The project developer.
    Developer,
}

impl Identity {
    /// Both identities, in the order the page offers them.
    pub const ALL: [Identity; 2] = [Identity::Buyer, Identity::Developer];

    /// The key the page sends to choose this identity.
    pub const fn key(self) -> &'static str {
        match self {
            Identity::Buyer => "buyer",
            Identity::Developer => "developer",
        }
    }

    /// What the switcher shows for this identity.
    ///
    /// A label, and nothing the client acts on: what this identity may hear is
    /// the server's answer, not this string's.
    pub const fn label(self) -> &'static str {
        match self {
            Identity::Buyer => "Buyer of unit 1204",
            Identity::Developer => "Project developer",
        }
    }

    /// The bearer token to authenticate with.
    ///
    /// Read from the environment so this simulator stays in step with a server
    /// started with non-default tokens, under the same two variable names the
    /// server reads.
    pub fn token(self) -> String {
        let (variable, fallback) = match self {
            Identity::Buyer => (BUYER_TOKEN_ENV, DEFAULT_BUYER_TOKEN),
            Identity::Developer => (DEVELOPER_TOKEN_ENV, DEFAULT_DEVELOPER_TOKEN),
        };
        match std::env::var(variable) {
            Ok(token) if !token.trim().is_empty() => token,
            _ => fallback.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_identity_has_a_distinct_key() {
        let keys: Vec<&str> = Identity::ALL.iter().map(|id| id.key()).collect();
        assert_eq!(keys, vec!["buyer", "developer"]);
    }

    #[test]
    fn an_identity_serialises_as_its_key() {
        for identity in Identity::ALL {
            let json = serde_json::to_string(&identity).expect("an identity serialises");
            assert_eq!(json, format!("\"{}\"", identity.key()));
        }
    }

    #[test]
    fn the_two_identities_carry_different_tokens() {
        assert_ne!(Identity::Buyer.token(), Identity::Developer.token());
    }
}
