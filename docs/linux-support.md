# Linux support audit

The current release builds a static Linux x86_64 musl binary, but the bundled Skill bootstrap and doctor scripts rejected Linux. Both now select the published Linux target. The doctor retains Skill-local binary isolation rather than falling back to an unrelated executable on PATH. Linux ARM64 remains unsupported by the release pipeline.

The loopback PKCE login previously had two unusable paths on headless Linux: browser-opening failure aborted login, and `--no-open` waited without exposing the URL. Login now emits its URL and SSH callback forwarding guidance to stderr before waiting; a missing opener falls back to manual authorization. Stdout remains machine-readable final JSON. This does not automate account consent or reuse another user's credentials.

Unrelated callbacks (including an incorrect state) no longer terminate the pending flow. The callback page acknowledges receipt and asks the user to confirm CLI completion, rather than claiming that credential exchange has already succeeded.

Validation covers a real CLI child process, loopback callback, local mock credential exchange, missing desktop opener, no-open login, and Linux bootstrap with checksum-verified fixtures. These tests do not substitute for a human completing real website consent on a Linux desktop or SSH workstation. No release is published by this change.
