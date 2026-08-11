# Troubleshooting

Run `browser-cli doctor` first and use the failed check's message.

- `configuration_error`: run `browser-cli auth login`, or verify the managed environment contains both required variables.
- `authentication_error`: the credential was rejected; log out and authorize again.
- `conflict`: a read-write Context is already locked. Use another Context, wait for the active session, or use read-only mode. Force-release only after confirming the session is dead.
- `timeout`: inspect session status and network access, then retry with a larger timeout.
- `cdp_error`: verify the session is active, inspect `session targets`, and take a snapshot before retrying the action.
- command not found after bootstrap: use the Skill-local binary under `${CODEBUDDY_SKILL_DIR}/bin/`; no PATH change or restart is required.

Always close a newly created temporary session when abandoning a failed task.
