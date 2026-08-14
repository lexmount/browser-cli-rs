# Command reference

Every command returns a JSON object with `ok` and either `data` or `error`.
The examples use `browser-cli` as shorthand for the Skill-local binary resolved
from the directory containing `SKILL.md`; invoke that binary by its absolute path.

```text
browser-cli doctor
browser-cli auth status
browser-cli auth login [--client-name "NAME"]

browser-cli session create [--browser-mode normal|light]
  [--context-id ID --context-mode read_write|read_only]
  [--downloads] [--recording] [--window-size 1920,1080]
browser-cli session list [--status active]
browser-cli session get --session-id ID
browser-cli session targets --session-id ID
browser-cli session keepalive --session-id ID [--duration 60]
browser-cli session close --session-id ID
browser-cli session downloads list --session-id ID
browser-cli session downloads get --session-id ID --download-id ID --output FILE
browser-cli session downloads archive --session-id ID --output FILE
browser-cli session downloads delete --session-id ID --yes

browser-cli context create [--description TEXT] [--metadata-json JSON]
browser-cli context list [--status available|locked] [--limit 20]
browser-cli context get --context-id ID
browser-cli context fork --context-id ID
browser-cli context delete --context-id ID --yes
browser-cli context force-release --context-id ID --yes

browser-cli action open-url --session-id ID --url URL
browser-cli action snapshot --session-id ID
browser-cli action wait-selector --session-id ID --selector CSS
browser-cli action wait-text --session-id ID --text "Saved" [--selector CSS]
browser-cli action wait-text --session-id ID --text "Saved" [--selector CSS] --exact
browser-cli action click --session-id ID --selector CSS
browser-cli action fill --session-id ID --selector CSS --value TEXT
browser-cli action screenshot --session-id ID --path FILE [--full-page]
browser-cli action pdf --session-id ID --path FILE [--print-background]
browser-cli action eval --session-id ID --expression JS
browser-cli action raw --session-id ID --method CDP_METHOD --params-json JSON
```

Use temporary sessions for public browsing. Use a dedicated persistent Context per account or purpose; avoid sharing one read-write Context between parallel tasks.

`wait-text` uses case-insensitive normalized contains matching by default. Add
`--exact` only when the entire normalized text must match. The obsolete
`--match contains` form is not supported.
