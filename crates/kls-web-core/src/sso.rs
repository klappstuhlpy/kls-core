//! Cross-app single sign-on handoff.
//!
//! A tiny, short-lived, HMAC-signed token that lets one app (the *issuer* — e.g.
//! klappstuhl.me) vouch a Discord identity to another (the *verifier* — e.g. the
//! Percy dashboard) **without a shared database or session store**. Both apps hold
//! the same [`SecretKey`] (the `sso_secret` in their configs); the issuer signs a
//! [`Handoff`] and redirects the browser to the verifier, which checks the
//! signature + freshness and mints its own native session.
//!
//! This is deliberately *not* a session — it is a one-time bearer token with a
//! very short lifetime (seconds), meant to be exchanged immediately on arrival.
//! It bridges the two apps' different identity models explicitly (the issuer's
//! account → its linked Discord user id) rather than trying to share one cookie.
//!
//! The signed form is URL-safe (base64url payload + signature), so it drops
//! straight into a redirect query parameter.

use serde::{Deserialize, Serialize};

use crate::key::SecretKey;

/// Default lifetime of a handoff token, in seconds. Short by design — the token
/// is exchanged for a session the instant the browser follows the redirect.
pub const DEFAULT_TTL_SECS: i64 = 60;

/// A signed vouch for a Discord identity, passed between apps in a redirect URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    /// The Discord user id the issuer has authenticated.
    #[serde(rename = "u")]
    pub discord_user_id: String,
    /// A best-effort display name for the user (the verifier may refine it).
    #[serde(rename = "n")]
    pub username: String,
    /// Unix-seconds expiry.
    #[serde(rename = "e")]
    pub expires: i64,
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Handoff {
    /// A fresh handoff for `discord_user_id`, valid for `ttl_secs` seconds.
    pub fn new(discord_user_id: impl Into<String>, username: impl Into<String>, ttl_secs: i64) -> Self {
        Self {
            discord_user_id: discord_user_id.into(),
            username: username.into(),
            expires: now_unix() + ttl_secs,
        }
    }

    /// Whether the token is still within its (short) lifetime.
    pub fn is_fresh(&self) -> bool {
        self.expires > now_unix()
    }

    /// Sign this handoff with the shared SSO key, producing the opaque, URL-safe
    /// token string to place in the redirect. `None` only on a serialisation
    /// error (never in practice for this fixed shape).
    pub fn sign(&self, key: &SecretKey) -> Option<String> {
        key.sign(self).ok()
    }

    /// Verify + decode a token with the shared SSO key, returning it **only** when
    /// the signature is valid *and* it has not expired.
    pub fn verify(key: &SecretKey, token: &str) -> Option<Self> {
        key.verify::<Self>(token).filter(Self::is_fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_round_trip() {
        let key = SecretKey::random().unwrap();
        let handoff = Handoff::new("1234567890", "ferris", DEFAULT_TTL_SECS);
        let token = handoff.sign(&key).expect("sign");

        let back = Handoff::verify(&key, &token).expect("verify");
        assert_eq!(back, handoff);
    }

    #[test]
    fn wrong_key_is_rejected() {
        let key = SecretKey::random().unwrap();
        let other = SecretKey::random().unwrap();
        let token = Handoff::new("1", "x", DEFAULT_TTL_SECS).sign(&key).unwrap();
        assert!(Handoff::verify(&other, &token).is_none());
    }

    #[test]
    fn expired_token_is_rejected() {
        let key = SecretKey::random().unwrap();
        // Already-expired handoff, but validly signed.
        let expired = Handoff {
            discord_user_id: "1".into(),
            username: "x".into(),
            expires: now_unix() - 1,
        };
        let token = expired.sign(&key).unwrap();
        assert!(Handoff::verify(&key, &token).is_none());
    }

    #[test]
    fn tampered_token_is_rejected() {
        let key = SecretKey::random().unwrap();
        let token = Handoff::new("1", "x", DEFAULT_TTL_SECS).sign(&key).unwrap();
        // Flip a character in the payload — the signature no longer matches.
        let mut bytes: Vec<char> = token.chars().collect();
        bytes[0] = if bytes[0] == 'A' { 'B' } else { 'A' };
        let tampered: String = bytes.into_iter().collect();
        assert!(Handoff::verify(&key, &tampered).is_none());
    }
}
