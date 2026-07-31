//! Password-based login, shared by every account and the admin role — see the
//! project design memory ("account-system-round2-auth-and-admin-design") for
//! the full picture. A few decisions worth knowing before touching this file:
//!
//! - Tokens are **opaque, server-tracked random strings, not JWT**. Every
//!   other permission check in this codebase (e.g. an account's own
//!   `allow_admin_readonly`, once that lands) is checked live against stored
//!   state, never baked into a cached claim — a signed token fights that, and
//!   revoking one needs its own blocklist anyway, which is the same
//!   server-side lookup an opaque token needs, minus a signing dependency
//!   this project doesn't otherwise have.
//! - Both the access and refresh token travel in the `Authorization` header,
//!   never a cookie — see [`crate::state::CurrentAccount`] for why (in short:
//!   it keeps `dev:api`, the frontend and backend on different origins,
//!   working without touching the CORS wildcard default).
//! - An account with no fixed password yet (nothing in `credentials.json`
//!   beyond a username) gets a fresh random password every boot, generated
//!   and logged the same way the admin account is — see
//!   [`AppState::account_by_id`]'s credential-seeding and
//!   [`generate_boot_password`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use serde::{Deserialize, Serialize};

use crate::models::now_ms;
use crate::persist::quarantine;

/// How long an access token is valid for. Short: a leaked one (a log line, a
/// devtools tab left open) stops working on its own soon after. Routine use
/// re-mints one from the refresh token before it expires.
const ACCESS_TOKEN_TTL_MS: i64 = 15 * 60 * 1000;
/// How long a refresh token is valid for — long enough that a session outlives
/// a browser restart without a password prompt every 15 minutes, still finite.
const REFRESH_TOKEN_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// The fixed login name for the admin role — it isn't a regular account (no
/// workspace, never created through account management), so it doesn't have
/// a `username` field to look up; this is the one constant every login
/// attempt is checked against first.
pub const ADMIN_USERNAME: &str = "admin";

/// Who a verified token belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    Admin,
    Account(String),
}


/// One account's own login fields, stored at `accounts/<id>/credentials.json`
/// — separate from `workspace.json` since this is identity, not the domain
/// data the workspace CRUD owns.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountCredentials {
    pub username: String,
    /// An Argon2 hash, or `None` when no fixed password has been set yet.
    /// [`generate_boot_password`] covers that case with a fresh, logged,
    /// per-boot password instead — the same treatment the admin account gets
    /// until an operator sets a real one.
    pub password_hash: Option<String>,
    /// Lets the admin view this account's data read-only. Defaults off; an
    /// account opts in from its own Settings (not built this round — the
    /// field exists now so the stored shape doesn't need to change later).
    #[serde(default)]
    pub allow_admin_readonly: bool,
}

/// Hashes a password with Argon2 and a fresh random salt.
pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("hashing a bounded-length password never fails")
        .to_string()
}

/// Checks a password against a stored Argon2 hash. A malformed stored hash
/// (should never happen — only [`hash_password`] ever writes one) is treated
/// as "does not match" rather than a panic.
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// An Argon2 hash of no password anyone will ever type, verified against on
/// every login for a username [`AppState::login`](crate::state::AppState::login)
/// can't find. Argon2 is deliberately slow (~tens of ms); a lookup that misses
/// used to return in microseconds while a real account's wrong-password
/// attempt paid that full cost, so measuring the response time alone told an
/// attacker whether a username existed — no password guessing needed.
/// Verifying against this dummy hash on the miss path burns the same cost as
/// the hit path, so the two are indistinguishable by timing.
pub static DUMMY_PASSWORD_HASH: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| hash_password("no account will ever have this exact password"));

/// A random password an operator can read off a log line and type in by
/// hand: alphanumeric only (no `+`/`/`/`=` to fumble), and excludes visually
/// ambiguous characters (`0`/`O`, `1`/`l`/`I`).
pub fn generate_boot_password() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    const LENGTH: usize = 20;
    let mut bytes = [0u8; LENGTH];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

/// A random opaque token — 32 bytes of CSPRNG output, hex-encoded. Used for
/// both access and refresh tokens; nothing about a token's own bytes says
/// which kind it is, only which map it's tracked in (see [`TokenStore`]).
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct TokenRecord {
    subject: Subject,
    expires_at: i64,
}

/// The live access/refresh tokens issued so far, entirely in-memory —
/// consistent with every token being minted fresh at login and this project's
/// per-boot admin/account password bootstrap: nothing here is meant to
/// survive a restart. Access and refresh tokens live in separate maps so a
/// refresh token (long-lived) can never be presented as an access token
/// (short-lived) to an ordinary route — only `/auth/refresh` ever reads the
/// refresh map.
#[derive(Default)]
pub struct TokenStore {
    access: Mutex<HashMap<String, TokenRecord>>,
    refresh: Mutex<HashMap<String, TokenRecord>>,
}

/// A freshly issued token pair.
pub struct IssuedTokens {
    pub access_token: String,
    pub refresh_token: String,
}

impl TokenStore {
    /// Issues a fresh access/refresh pair for `subject`, discarding nothing
    /// else already issued — a login from a second device/tab is a second,
    /// independent session, not a replacement for the first.
    pub fn issue(&self, subject: Subject) -> IssuedTokens {
        let now = now_ms();
        let access_token = generate_token();
        self.access.lock().unwrap().insert(
            access_token.clone(),
            TokenRecord { subject: subject.clone(), expires_at: now + ACCESS_TOKEN_TTL_MS },
        );
        let refresh_token = generate_token();
        self.refresh.lock().unwrap().insert(
            refresh_token.clone(),
            TokenRecord { subject, expires_at: now + REFRESH_TOKEN_TTL_MS },
        );
        IssuedTokens { access_token, refresh_token }
    }

    /// The subject an access token currently resolves to, or `None` for an
    /// unknown or expired token. Expired entries are lazily dropped here
    /// rather than swept on a timer — nothing reads a token after it stops
    /// verifying, so there's nothing to save by cleaning it up sooner.
    pub fn verify_access(&self, token: &str) -> Option<Subject> {
        let mut access = self.access.lock().unwrap();
        let record = access.get(token)?;
        if record.expires_at < now_ms() {
            access.remove(token);
            return None;
        }
        Some(record.subject.clone())
    }

    /// Mints a fresh access token for whoever `refresh_token` belongs to,
    /// without rotating the refresh token itself. `None` for an unknown,
    /// expired, or already-revoked refresh token.
    pub fn refresh(&self, refresh_token: &str) -> Option<String> {
        let mut refresh = self.refresh.lock().unwrap();
        let record = refresh.get(refresh_token)?;
        if record.expires_at < now_ms() {
            refresh.remove(refresh_token);
            return None;
        }
        let subject = record.subject.clone();
        drop(refresh);
        let now = now_ms();
        let access_token = generate_token();
        self.access.lock().unwrap().insert(
            access_token.clone(),
            TokenRecord { subject, expires_at: now + ACCESS_TOKEN_TTL_MS },
        );
        Some(access_token)
    }
}

/// The admin's own credential file, `admin.json` at the data directory root —
/// a sibling to `llm.toml`, since like it this is operator-level config, not
/// per-account data. Absent, or present with `passwordHash: null`, means "no
/// fixed password set yet" — [`crate::state::AppState::with_admin_auth`]
/// generates and logs a fresh one for that boot in that case, the same
/// treatment an account with no fixed password gets. There is no writer here
/// yet since nothing can set a fixed admin password this round (that needs
/// account management, a later round) — hand-editing the file with a real
/// Argon2 hash works today the same way `llm.toml` is hand-editable.
pub struct AdminConfigStore {
    path: PathBuf,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdminConfig {
    password_hash: Option<String>,
}

impl AdminConfigStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { path: data_dir.into().join("admin.json") }
    }

    /// The stored password hash, or `None` when there isn't one yet — no
    /// file, or a corrupt one, quarantined (not silently discarded) since
    /// this holds a credential, not a readout that's cheap to reset.
    pub fn load_password_hash(&self) -> Option<String> {
        let bytes = std::fs::read(&self.path).ok()?;
        match serde_json::from_slice::<AdminConfig>(&bytes) {
            Ok(config) => config.password_hash,
            Err(e) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %e,
                    "admin.json is unreadable; preserving it and starting from a fresh boot password"
                );
                quarantine(&self.path);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_password_verifies_and_rejects_a_wrong_one() {
        let hash = hash_password("correct horse battery staple");
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn a_malformed_stored_hash_fails_closed() {
        assert!(!verify_password("anything", "not a real hash"));
    }

    #[test]
    fn access_token_verifies_immediately_after_issue() {
        let store = TokenStore::default();
        let issued = store.issue(Subject::Account("acct-1".to_string()));
        assert_eq!(store.verify_access(&issued.access_token), Some(Subject::Account("acct-1".to_string())));
    }

    #[test]
    fn unknown_token_does_not_verify() {
        let store = TokenStore::default();
        assert_eq!(store.verify_access("not-a-real-token"), None);
    }

    #[test]
    fn refresh_token_mints_a_new_working_access_token() {
        let store = TokenStore::default();
        let issued = store.issue(Subject::Admin);
        let fresh = store.refresh(&issued.refresh_token).expect("a valid refresh token");
        assert_eq!(store.verify_access(&fresh), Some(Subject::Admin));
    }

    #[test]
    fn an_access_token_is_not_accepted_as_a_refresh_token() {
        let store = TokenStore::default();
        let issued = store.issue(Subject::Admin);
        assert!(store.refresh(&issued.access_token).is_none());
    }

    #[test]
    fn boot_passwords_and_tokens_are_reasonably_unpredictable() {
        // Not a rigorous entropy test — just a guard against a regression
        // that accidentally makes generation deterministic or constant.
        assert_ne!(generate_boot_password(), generate_boot_password());
        assert_ne!(generate_token(), generate_token());
        assert_eq!(generate_boot_password().chars().count(), 20);
    }
}
