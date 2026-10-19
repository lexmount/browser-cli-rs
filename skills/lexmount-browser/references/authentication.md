# Authentication

Preferred login:

```text
browser-cli auth login [--client-name "NAME"]
```

The default client name is `Agent`. Quote and pass the current Agent's
user-facing name when available; for example, WorkBuddy can pass
`--client-name "WorkBuddy"`.

The CLI binds a random loopback port on `127.0.0.1`, creates a PKCE verifier and state, opens the Lexmount approval page, exchanges the returned one-time code, and stores the scoped credential in:

```text
~/.config/lexmount/browser-cli/credentials.json
```

The file is mode `0600` on Unix. The CLI redacts the API key from all JSON output.

For managed environments, the SDK also accepts `LEXMOUNT_API_KEY`, `LEXMOUNT_PROJECT_ID`, optional `LEXMOUNT_BASE_URL`, and optional `LEXMOUNT_REGION`. Do not ask users to paste secret values into an Agent chat.

Use `browser-cli auth logout` to remove only the local credential file. Environment variables are managed outside the CLI.

## Account supplied by the desktop app (macOS)

After login, the native Lexmount app enables its account selector at
`~/.config/lexmount/browser-cli/desktop-auth.json` (`{"version":1,"enabled":true}`).
Default CLI and Rust SDK clients then read the classic macOS Keychain generic
password with service `com.lexmount.tab-manager.auth`, account `active`. The
value is the existing Credentials JSON; the selector contains no credential.
The app explicitly grants Keychain read access to itself and its bundled CLI.
An arbitrary SDK host does not inherit that permission. The reader never
changes ACLs or invokes a command-line password utility, and it fails rather
than presenting an interactive Keychain permission prompt.

Once enabled, a missing, locked, denied, invalid or expired desktop credential
does **not** fall back to an old CLI credential file. Signing out in the app
removes the Keychain item but retains the selector. `auth status` reports
`source:"desktop_keychain"`, `valid:false` and a fixed `error_code` when the
account is unavailable; it does not print Keychain data or OS error details.
Only a known RFC3339 expiration is checked locally; absent expiration does not
prove the server has not revoked the key. Server authorization remains decisive.

The app pins a complete account with `LEXMOUNT_AUTH_SOURCE=desktop` plus the pair
`LEXMOUNT_DESKTOP_EXPECTED_PROJECT_ID` and
`LEXMOUNT_DESKTOP_EXPECTED_API_BASE_URL`. The reader requires the returned
project and canonical HTTPS API origin to match. Desktop credentials accept
only the official `api.lexmount.com` and `api.lexmount.cn` origins. Conflicting
credential files or per-field identity overrides fail, rather than combining a
desktop key with another project or host.

An explicit credential file (`--credentials-file` where supported, or
`LEXMOUNT_BROWSER_CREDENTIALS_FILE`) still selects an isolated file account when
desktop mode is not forced. A complete explicit SDK/environment key and project
also selects the caller's own account, without reading the desktop Keychain;
set its API base explicitly when using a nondefault environment. Partial
identity/host overrides cannot be filled from the desktop account. Existing
file-mode behavior remains available when the selector is absent or disabled.

With desktop mode enabled and no explicit file, `auth login` and `auth logout`
instruct the caller to manage that account in the Lexmount app; they do not
write/delete an unrelated legacy file or claim to revoke a server key. CLI
logout with an explicit file removes only that file. Desktop account creation,
selection, ACL management and sign-out remain owned by the native app.
