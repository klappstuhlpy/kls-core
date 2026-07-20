//! Shared web kernel for klappstuhl.me, the Percy dashboard, and Vantage.
//!
//! Holds the state-agnostic foundation those binaries build on. Everything here
//! is free of app-specific types — no `AppState`, `Config`, or domain models —
//! which is what lets one kernel serve three unrelated applications.
//!
//! Modules:
//! - [`key`] — HMAC-SHA256 signing/verification over a 32-byte [`key::SecretKey`],
//!   plus hex helpers. Backs signed cookies, tokens, and TOTP secrets.
//! - [`database`] — the async SQLite wrapper (thread-pool over blocking
//!   `rusqlite` connections), the [`database::Table`] row-mapping trait, and the
//!   [`boxed_params`] helper macro.
//! - [`migrations`] — the generic gapless-from-0 SQLite migration runner
//!   ([`migrations::run`]) plus [`migrations::EmbeddedMigration`]. The build-time
//!   discovery of a specific app's migration set stays in that app.
//! - [`token`] — the signed auth-cookie wire format ([`token::Token`]) both apps
//!   mint and verify (the basis for cross-app SSO). Resolving a token to a
//!   concrete account stays app-side.
//! - [`sso`] — the cross-app single-sign-on handoff ([`sso::Handoff`]): a
//!   short-lived, shared-key-signed vouch of a Discord identity that lets one app
//!   log a user into the other without a shared database or session store.
//! - [`flash`] — signed, redirect-surviving flash messages ([`flash::Flasher`] /
//!   [`flash::Flashes`] + the `process_flash_messages` middleware).
//! - [`audit`] — an app-neutral audit-log writer/reader over a [`Database`]
//!   handle. Each app points it at its own DB and wraps it with app-specific
//!   side effects (live publishing, an `AppState::audit()` builder).

pub mod audit;
pub mod database;
pub mod flash;
pub mod key;
pub mod migrations;
pub mod sso;
pub mod token;

pub use database::Database;
