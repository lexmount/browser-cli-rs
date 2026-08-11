# Lexmount Browser CLI (Rust)

Native Rust SDK and command-line client for Lexmount cloud browsers. The binary is
named `browser-cli` so existing agent instructions can migrate without changing
their command prefix.

## Build

```bash
cargo build --release
./target/release/browser-cli version
```

Configuration is loaded from `LEXMOUNT_API_KEY`, `LEXMOUNT_PROJECT_ID`, optional
`LEXMOUNT_BASE_URL` (default `https://api.lexmount.cn`), and optional
`LEXMOUNT_REGION`. `browser-cli auth login` uses a loopback callback and PKCE;
credentials are stored at `~/.config/lexmount/browser-cli/credentials.json` with
mode `0600` on Unix and are never printed.

All commands emit one JSON document. Run `browser-cli --help` for the complete
surface.

## WorkBuddy package

The publishable Skill is in `skills/lexmount-browser`. Build a deterministic ZIP:

```bash
./scripts/package-skill.sh
```

The package installer downloads a pinned GitHub Release binary and verifies its
SHA-256 digest before installation. Set `LEXMOUNT_BROWSER_CLI_VERSION` only when
testing a different published release.
