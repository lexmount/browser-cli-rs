# Cloud device authorization

`browser auth login` calls the Global console's `/api/connect/device/authorize`.
Only the verification URL and human short code are returned to the conversation.
The private device code stays in a mode-0600 file in a mode-0700 state directory.
The user opens the URL on another device, signs into Lexmount and explicitly
approves the project and scopes. The cloud workspace then uses `auth poll` to call
`/api/connect/device/token` and receive its own scoped credential.

Login start, status, poll and cancel are separate short commands. No loopback
listener, port forwarding, local-token copying or persistent background process is
used. Polling honors the service interval, `slow_down`, temporary network failures,
denial and expiry. Repeated start calls reuse the existing pending code.

The result is stored in the existing native CLI credential format at:

```text
~/.config/lexmount/browser-cli/sites/global-cloud/credentials.json
```

For a private persistent cloud mount, set `LEXMOUNT_BROWSER_CREDENTIALS_FILE` to an
absolute file path before every invocation. Keep it isolated per user/workspace;
never put it in a shared repository or plugin directory. Separate credential files
also have separate pending authorizations. A fresh sandbox without that mount needs
new authorization. The plugin never imports credentials from a local computer.

Both console and API are fixed to `browser.lexmount.com` and `api.lexmount.com`.
Foreign-site credentials and API-key environment overrides are rejected. The
standalone CLI and its other installations are unaffected. While reauthorization
is pending, browser operations stop rather than using the previous identity.
Only successful approval replaces an existing credential. Denial, timeout, a
project mismatch, malformed response or cancellation leaves it intact.

`auth status` reports local state, not server acceptance. `doctor` verifies API
access. Logout deletes this cloud profile's local credential and cancels its
pending authorization; it does not revoke other clients' keys on the server.
