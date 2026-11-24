# Common native CLI commands

Use `sh "<skill-root>/scripts/browser.sh"` instead of the abbreviated `browser`
below. Commands return JSON; an exit code alone does not prove the intended result.

```text
browser auth login --client-name "Codex Cloud"
browser auth status
browser auth poll
browser auth cancel
browser auth logout
browser doctor

browser session create
browser session get --session-id ID
browser session targets --session-id ID
browser session close --session-id ID

browser action open-url --session-id ID --url https://example.com
browser action snapshot --session-id ID --target-id PAGE_ID
browser action fill --session-id ID --target-id PAGE_ID --selector CSS --value TEXT
browser action click --session-id ID --target-id PAGE_ID --selector CSS
browser action wait-selector --session-id ID --target-id PAGE_ID --selector CSS
browser action screenshot --session-id ID --target-id PAGE_ID --path /absolute/output.png
browser action pdf --session-id ID --target-id PAGE_ID --path /absolute/output.pdf

browser context create --description "Task account"
browser session create --context-id CONTEXT_ID --context-mode read_write
```

Read `action --help`, `session --help` or `context --help` before advanced commands.
Use the current snapshot's selectors and current page target ID. A successful
action receipt does not establish business success. Check actual page state;
never automatically retry a non-idempotent action after a failed/uncertain reply.

Screenshot and PDF commands save files in the cloud workspace; return the actual
file through the host's artifact support. Do not invent download URLs. If the
host cannot expose the Inspector link for manual website sign-in, report the
limitation and retain the existing session until the user cancels or takes over.
