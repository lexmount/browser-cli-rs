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


### Linux and SSH login

Linux x86_64 uses the static musl release and the same `scripts/bootstrap.sh` and `scripts/doctor.sh` as macOS. Linux ARM64 is not currently a published target.

`browser-cli auth login --no-open` prints the authorization URL on stderr and waits for the callback; stdout remains the final JSON result. The default login attempts to open a browser, but keeps waiting if the desktop opener is missing. On a headless/SSH host, forward the printed callback port from your desktop using `ssh -L PORT:127.0.0.1:PORT HOST`, then open the printed URL on your desktop. Keep the login process running until it completes. Do not post authorization URLs or credentials in chat.

## Device authorization

Use `browser-cli auth login --device` on a machine without a desktop browser.
Ask the human to open the displayed console URL on another device and enter the
short code. The CLI waits for explicit approval, then stores the credential. Never
ask them to send their password or API key in chat. This requires the console's
`/api/connect/device/authorize` and `/api/connect/device/token` endpoints.

SSH and headless Linux choose this automatically; `--browser` forces loopback
login, while `--no-open` preserves manual loopback login. CI should use preconfigured
credentials. Denial or expiry stops login; do not silently retry a denied request.
