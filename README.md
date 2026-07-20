# kls-core

The versioned foundation shared by [Vantage](https://github.com/klappstuhlpy/vantage),
the klappstuhl.me main site, and the Percy dashboard. Four small, independent
crates.

| Crate | What it is |
|---|---|
| `kls-web-core` | The web kernel: HMAC signing over a 32-byte key, an async SQLite wrapper over a blocking-connection thread pool, a gapless migration runner, the signed auth-cookie wire format, cross-app SSO handoffs, redirect-surviving flash messages, and an app-neutral audit writer |
| `kls-agent` | A typed, allowlisted boundary for privileged host operations — firewall, docker, proxy — so mutations go through one reviewable surface instead of ad-hoc shell |
| `percy-client` | A typed client for Percy's internal HTTP API. Describes request and response shapes only; every call is bearer-authenticated with a token the consuming app holds |
| `kls-ui` | The design system shared by the site and the Percy dashboard — base stylesheet and two JS utilities, embedded at compile time and served under `/kls/*`. Vantage deliberately does not use it; it ships its own look |

None carry app-specific types: no `AppState`, no `Config`, no domain models.
They are consumed as tag-pinned git dependencies rather than published to
crates.io, because their versioning tracks the applications that use them:

```toml
kls-web-core = { git = "https://github.com/klappstuhlpy/kls-core", tag = "v0.1.0" }
kls-agent = { git = "https://github.com/klappstuhlpy/kls-core", tag = "v0.1.0" }
```

## Why this repo is public

Vantage is AGPL-v3 and links these crates statically, so anyone receiving a
Vantage binary is entitled to the corresponding source of the combined work.
Distributing a public image built from private sources cannot satisfy that — and
for a security tool, an unbuildable binary is a credibility problem beyond the
licensing one.

## Development

```bash
cargo test    # both crates
cargo fmt     # max_width = 120, Unix newlines
cargo clippy
```

## License

AGPL-3.0-or-later. See [LICENSE](LICENSE).
