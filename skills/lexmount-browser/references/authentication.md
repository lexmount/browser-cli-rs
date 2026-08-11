# Authentication

Preferred login:

```text
browser-cli auth login
```

The CLI binds a random loopback port on `127.0.0.1`, creates a PKCE verifier and state, opens the Lexmount approval page, exchanges the returned one-time code, and stores the scoped credential in:

```text
~/.config/lexmount/browser-cli/credentials.json
```

The file is mode `0600` on Unix. The CLI redacts the API key from all JSON output.

For managed environments, the SDK also accepts `LEXMOUNT_API_KEY`, `LEXMOUNT_PROJECT_ID`, optional `LEXMOUNT_BASE_URL`, and optional `LEXMOUNT_REGION`. Do not ask users to paste secret values into WorkBuddy chat.

Use `browser-cli auth logout` to remove only the local credential file. Environment variables are managed outside the CLI.
