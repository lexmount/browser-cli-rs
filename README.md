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
mkdir -p skills/lexmount-browser/bin
cp /path/to/macos-arm64/browser-cli skills/lexmount-browser/bin/browser-cli
./scripts/package-skill.sh
```

The ZIP contains `SKILL.md` at its archive root plus the signed macOS arm64
binary at `bin/browser-cli`. It deliberately excludes the unsigned Windows x64
executable so SkillHub does not reject or strip the package. Tagged releases
publish it as `lexmount-browser-v<VERSION>-skillhub.zip` alongside the standalone
platform binaries and `SHA256SUMS`.

The Skill uses the bundled binary on macOS arm64. On Windows x64,
`bootstrap.ps1` downloads the pinned GitHub Release executable and verifies its
SHA-256 digest before use. The macOS bootstrap remains a missing-binary fallback.
Set `LEXMOUNT_BROWSER_CLI_VERSION` only when testing a different published
release.

Published binaries are intentionally limited to two targets: macOS arm64 and
Windows x64. The macOS binary is signed with a Developer ID Application
certificate, hardened-runtime enabled, and accepted by Apple's notarization
service before it is published. Linux and macOS Intel remain unsupported
release platforms.

The release workflow reads the signing certificate and notarization credentials
from the `macos-release` GitHub environment. It requires
`MACOS_DEVELOPER_ID_APPLICATION_P12_BASE64`,
`MACOS_DEVELOPER_ID_P12_PASSWORD`, `APPLE_NOTARY_APPLE_ID`,
`APPLE_NOTARY_TEAM_ID`, and `APPLE_NOTARY_APP_PASSWORD`.
