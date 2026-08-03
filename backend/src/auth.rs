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

/// A refresh token this store already exchanged once, kept just long enough
/// to answer a second, racing presentation of it the same way instead of
/// refusing it — see [`TokenStore::refresh`].
struct RetiredRefresh {
    replacement: String,
    subject: Subject,
    retired_at: i64,
}

/// How long a rotated-out refresh token is still honoured as a stand-in for
/// its replacement. Long enough to cover a genuine race (two tabs sharing one
/// refresh token, a client retrying a request whose response it never saw);
/// short enough that it isn't a second, shadow validity window. Shrunk in
/// tests so "past the window" is a `sleep`, not a slow test.
#[cfg(not(test))]
const REUSE_GRACE_MS: i64 = 10_000;
// 500ms rather than something tighter: `cargo test` runs suites in parallel,
// and a shared, loaded CI runner can plausibly deschedule a test thread for
// tens of milliseconds between two back-to-back calls with no sleep between
// them — which would turn "a racing second use" into a flaky "past the
// window" failure. 500ms is short enough that the grace-expiry test's sleep
// stays fast.
#[cfg(test)]
const REUSE_GRACE_MS: i64 = 500;

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
    retired_refresh: Mutex<HashMap<String, RetiredRefresh>>,
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

    /// Drops exactly the tokens presented — one session's own sign-out.
    ///
    /// Deliberately takes no authorization of its own: possessing a token is
    /// what entitles you to destroy it, and requiring a *valid* one would mean
    /// an expired access token left its refresh token (good for a month) alive
    /// with no way to retract it. Unknown tokens are silently ignored, so this
    /// can't be used to probe which tokens exist.
    pub fn revoke_tokens(&self, access: Option<&str>, refresh: Option<&str>) {
        if let Some(token) = access {
            self.access.lock().unwrap().remove(token);
        }
        if let Some(token) = refresh {
            self.refresh.lock().unwrap().remove(token);
        }
    }

    /// Drops every access and refresh token belonging to `subject`, ending all
    /// of its live sessions at once.
    ///
    /// Called when an account's credentials change. Without it, changing a
    /// password did nothing to whoever was already holding a token for that
    /// account: their access token kept working until it expired, and their
    /// refresh token — good for 30 days — kept minting new ones. "Change the
    /// password" is the one lever an operator has when an account is
    /// compromised, so it has to actually cut off the existing session rather
    /// than only the next login.
    ///
    /// Every session of that subject goes, including the one that may have
    /// asked for the change; re-logging in with the new password is the
    /// intended follow-up.
    pub fn revoke_subject(&self, subject: &Subject) {
        self.access.lock().unwrap().retain(|_, record| record.subject != *subject);
        self.refresh.lock().unwrap().retain(|_, record| record.subject != *subject);
    }

    /// Mints a fresh access/refresh pair for whoever `refresh_token` belongs
    /// to, and retires `refresh_token` itself — it stops working the instant
    /// its replacement exists, same as any other rotated credential. `None`
    /// for a refresh token that's unknown, expired, or was rotated out longer
    /// ago than [`REUSE_GRACE_MS`].
    //
    // Rotation without this project having cross-tab token sync yet is what
    // makes the grace window necessary: two tabs sharing one refresh token
    // (only one physically talks to `/auth/refresh` first) would otherwise
    // have the second tab treated as presenting a dead token. A presentation
    // past the grace window is just denied, not treated as a theft signal
    // that revokes every session for the subject — this store has no way to
    // tell "a stale background tab finally woke up" apart from "someone
    // replayed a captured token," and guessing wrong signs a real user out
    // for no reason. Rotation alone already closes the actual gap (a
    // captured refresh token used to stay valid, unrotated, for its whole
    // 30-day life); reuse *detection* is a further hardening step that needs
    // that sync built first.
    pub fn refresh(&self, refresh_token: &str) -> Option<(IssuedTokens, Subject)> {
        self.prune_retired();
        {
            let mut refresh = self.refresh.lock().unwrap();
            if let Some(record) = refresh.get(refresh_token) {
                if record.expires_at < now_ms() {
                    refresh.remove(refresh_token);
                    return None;
                }
                let subject = record.subject.clone();
                refresh.remove(refresh_token);
                drop(refresh);
                let issued = self.rotate(refresh_token, subject.clone());
                return Some((issued, subject));
            }
        }
        let retired = self.retired_refresh.lock().unwrap();
        let record = retired.get(refresh_token)?;
        if now_ms() - record.retired_at > REUSE_GRACE_MS {
            return None;
        }
        let replacement = record.replacement.clone();
        let subject = record.subject.clone();
        drop(retired);
        // The replacement itself may have been revoked since rotation (a
        // password change, an explicit logout) — honouring a grace-window
        // replay in that case would resurrect a session that was
        // deliberately ended.
        if !self.refresh.lock().unwrap().contains_key(&replacement) {
            return None;
        }
        let access_token = generate_token();
        self.access.lock().unwrap().insert(
            access_token.clone(),
            TokenRecord { subject: subject.clone(), expires_at: now_ms() + ACCESS_TOKEN_TTL_MS },
        );
        Some((IssuedTokens { access_token, refresh_token: replacement }, subject))
    }

    /// Mints the replacement pair for `old_token` and records the retirement.
    fn rotate(&self, old_token: &str, subject: Subject) -> IssuedTokens {
        let now = now_ms();
        let access_token = generate_token();
        self.access.lock().unwrap().insert(
            access_token.clone(),
            TokenRecord { subject: subject.clone(), expires_at: now + ACCESS_TOKEN_TTL_MS },
        );
        let new_refresh = generate_token();
        self.refresh.lock().unwrap().insert(
            new_refresh.clone(),
            TokenRecord { subject: subject.clone(), expires_at: now + REFRESH_TOKEN_TTL_MS },
        );
        self.retired_refresh.lock().unwrap().insert(
            old_token.to_string(),
            RetiredRefresh { replacement: new_refresh.clone(), subject, retired_at: now },
        );
        IssuedTokens { access_token, refresh_token: new_refresh }
    }

    /// Drops retirement records past their grace window — lazily, on the same
    /// "nothing sweeps on a timer" principle [`Self::verify_access`] already
    /// follows, since this map is only ever consulted from [`Self::refresh`].
    fn prune_retired(&self) {
        let now = now_ms();
        self.retired_refresh.lock().unwrap().retain(|_, r| now - r.retired_at <= REUSE_GRACE_MS);
    }
}

struct LoginAttempts {
    failures: u32,
    /// Set once `failures` crosses [`LoginThrottle::FREE_ATTEMPTS`]; an
    /// absolute timestamp, not a duration, so [`LoginThrottle::retry_after_secs`]
    /// only has to compare against "now" rather than re-derive one.
    retry_after: Option<i64>,
}

/// Slows down online password guessing against `POST /auth/login`.
///
/// Keyed on the literal username *typed in*, not on whether it belongs to a
/// real account — a nonexistent username gets throttled exactly like a real
/// one, so presence or absence of a lockout can't be used to probe which
/// usernames exist (the same concern [`DUMMY_PASSWORD_HASH`] exists for, just
/// on the throughput axis instead of the timing one). Deliberately not a hard
/// lockout: a fixed, short `Retry-After` that never compounds into something
/// unbounded, because `ADMIN_USERNAME` is a fixed, guessable string — an
/// attacker who can make the admin account unusable for minutes at a time by
/// failing its password a few times is its own denial-of-service.
#[derive(Default)]
pub struct LoginThrottle {
    by_username: Mutex<HashMap<String, LoginAttempts>>,
}

impl LoginThrottle {
    /// Failures below this many don't slow anything down — a mistyped
    /// password shouldn't cost a real user a wait.
    const FREE_ATTEMPTS: u32 = 2;
    const RETRY_AFTER_MS: i64 = 8_000;
    /// A hard ceiling on distinct usernames tracked at once. A flood of
    /// requests carrying different junk usernames could otherwise grow this
    /// map without bound; hitting the cap only happens under an actual
    /// attack, so clearing it outright and starting over is an acceptable,
    /// rare cost rather than a bookkeeping scheme for something this small.
    const MAX_TRACKED: usize = 10_000;

    /// `Some(seconds)` if `username` must wait before its password is even
    /// checked.
    pub fn retry_after_secs(&self, username: &str) -> Option<u32> {
        let now = now_ms();
        let map = self.by_username.lock().unwrap();
        let until = map.get(username)?.retry_after?;
        (until > now).then(|| ((until - now + 999) / 1000) as u32)
    }

    pub fn record_failure(&self, username: &str) {
        let now = now_ms();
        let mut map = self.by_username.lock().unwrap();
        if map.len() >= Self::MAX_TRACKED && !map.contains_key(username) {
            map.clear();
        }
        let attempts =
            map.entry(username.to_string()).or_insert(LoginAttempts { failures: 0, retry_after: None });
        attempts.failures += 1;
        if attempts.failures > Self::FREE_ATTEMPTS {
            attempts.retry_after = Some(now + Self::RETRY_AFTER_MS);
        }
    }

    /// Clears any record for `username` — a successful login means whatever
    /// came before it doesn't matter anymore.
    pub fn record_success(&self, username: &str) {
        self.by_username.lock().unwrap().remove(username);
    }
}

#[cfg(test)]
mod login_throttle_tests {
    use super::*;

    #[test]
    fn the_first_two_failures_are_free() {
        let throttle = LoginThrottle::default();
        throttle.record_failure("alice");
        throttle.record_failure("alice");
        assert_eq!(throttle.retry_after_secs("alice"), None);
    }

    #[test]
    fn the_third_failure_starts_a_short_wait() {
        let throttle = LoginThrottle::default();
        for _ in 0..3 {
            throttle.record_failure("alice");
        }
        let wait = throttle.retry_after_secs("alice").expect("throttled after 3 failures");
        assert!(wait > 0 && wait <= 8);
    }

    #[test]
    fn a_success_clears_the_throttle() {
        let throttle = LoginThrottle::default();
        for _ in 0..3 {
            throttle.record_failure("alice");
        }
        throttle.record_success("alice");
        assert_eq!(throttle.retry_after_secs("alice"), None);
    }

    #[test]
    fn usernames_are_throttled_independently() {
        let throttle = LoginThrottle::default();
        for _ in 0..3 {
            throttle.record_failure("alice");
        }
        assert_eq!(throttle.retry_after_secs("bob"), None);
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
        let (fresh, subject) = store.refresh(&issued.refresh_token).expect("a valid refresh token");
        assert_eq!(subject, Subject::Admin);
        assert_eq!(store.verify_access(&fresh.access_token), Some(Subject::Admin));
    }

    #[test]
    fn an_access_token_is_not_accepted_as_a_refresh_token() {
        let store = TokenStore::default();
        let issued = store.issue(Subject::Admin);
        assert!(store.refresh(&issued.access_token).is_none());
    }

    #[test]
    fn refreshing_rotates_the_refresh_token() {
        let store = TokenStore::default();
        let issued = store.issue(Subject::Admin);
        let (fresh, _) = store.refresh(&issued.refresh_token).expect("a valid refresh token");
        assert_ne!(fresh.refresh_token, issued.refresh_token);
        // The new pair actually works.
        assert_eq!(store.verify_access(&fresh.access_token), Some(Subject::Admin));
        assert!(store.refresh(&fresh.refresh_token).is_some());
    }

    #[test]
    fn a_racing_second_use_of_a_just_rotated_token_gets_the_same_replacement() {
        // Two tabs sharing one refresh token: only one physically wins the
        // race to rotate it first. The second must not be treated as reusing
        // a dead token — it should land on the same replacement the first
        // one got, not be locked out by its own session's rotation.
        let store = TokenStore::default();
        let issued = store.issue(Subject::Admin);
        let (first, _) = store.refresh(&issued.refresh_token).expect("first use rotates it");
        let (second, _) = store.refresh(&issued.refresh_token).expect("a racing second use, within grace");
        assert_eq!(first.refresh_token, second.refresh_token);
        assert_eq!(store.verify_access(&second.access_token), Some(Subject::Admin));
    }

    #[test]
    fn a_rotated_token_is_refused_once_the_grace_window_passes() {
        let store = TokenStore::default();
        let issued = store.issue(Subject::Admin);
        store.refresh(&issued.refresh_token).expect("rotates it");
        std::thread::sleep(std::time::Duration::from_millis(REUSE_GRACE_MS as u64 + 100));
        assert!(store.refresh(&issued.refresh_token).is_none());
    }

    #[test]
    fn a_grace_window_replay_does_not_resurrect_a_revoked_session() {
        let store = TokenStore::default();
        let issued = store.issue(Subject::Account("acct-1".to_string()));
        store.refresh(&issued.refresh_token).expect("rotates it");
        store.revoke_subject(&Subject::Account("acct-1".to_string()));
        // The old token is still within its grace window, but its
        // replacement was just revoked (e.g. a password change) — honouring
        // the replay would undo that revocation.
        assert!(store.refresh(&issued.refresh_token).is_none());
    }

    #[test]
    fn revoking_a_subject_kills_both_token_kinds_and_leaves_others_alone() {
        let store = TokenStore::default();
        let alice = store.issue(Subject::Account("acct-1".to_string()));
        // A second session for the same account — a second device — must go too.
        let alice_phone = store.issue(Subject::Account("acct-1".to_string()));
        let bob = store.issue(Subject::Account("acct-2".to_string()));

        store.revoke_subject(&Subject::Account("acct-1".to_string()));

        assert_eq!(store.verify_access(&alice.access_token), None);
        assert_eq!(store.verify_access(&alice_phone.access_token), None);
        // The refresh token is the one that matters: it outlives the access
        // token by a month, so a revocation that missed it would barely help.
        assert!(store.refresh(&alice.refresh_token).is_none());
        assert!(store.refresh(&alice_phone.refresh_token).is_none());

        assert!(store.verify_access(&bob.access_token).is_some());
        assert!(store.refresh(&bob.refresh_token).is_some());
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
