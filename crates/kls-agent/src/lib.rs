//! kls-agent — the typed, allowlisted boundary for privileged host operations.
//!
//! The admin shell manages the host by shelling out to system tools (`nft`,
//! `ufw`, `iptables`, `docker`, `df`, an operator-configured proxy reload, cron
//! scripts). Before this crate those `Command::new(...)` calls were scattered
//! across the admin feature slices. Concentrating them here gives the split (see
//! `.claude/ADMIN_SEPARATION_PLAN.md` §7.2) a single, reviewable privileged
//! surface with a hard rule:
//!
//! * **The program is typed.** The safe path can only ever spawn a binary named
//!   by the [`Tool`] enum — a caller cannot pass an arbitrary program name.
//!   Arguments are an argv array (never a shell string), so nothing a caller
//!   puts in an argument is interpreted by a shell.
//! * **The one shell escape hatch is named and isolated.** [`exec::shell`] is
//!   the single unconstrained path; it exists only for operator-configured
//!   command strings (the proxy reload command) and admin cron scripts, which
//!   are arbitrary by nature. Keeping it here means every process spawn in the
//!   app funnels through kls-agent, so the dangerous path is greppable and ready
//!   for the later broker / sudo-mode gating.
//!
//! Step A of Phase 3 keeps kls-agent's transport in-process (it spawns the tools
//! directly). Step B swaps that transport for a D-Bus/broker client behind the
//! same API, without changing callers.

pub mod exec;

pub use exec::{shell, ExecError, HostCommand, Tool};
