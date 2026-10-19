# Command reference

Operation commands return a JSON object with `ok` and either `data` or `error`;
Clap help is plain text. Runtime failures are written to stderr, so retain the
error channel rather than treating empty stdout as an empty successful result.
The examples use `browser-cli` as shorthand for the Skill-local binary resolved
from the directory containing `SKILL.md`; invoke that binary by its absolute path.

## Check the installed capability

Read only the help needed for the requested operation. The source checkout,
package version and this reference are not evidence that the installed binary
contains a recently added command.

| Requested feature | Actual help to check |
| --- | --- |
| Explicit page | `action --help` lists `--target-id` |
| Real movement | `action move --help` lists `--from-json` and `--duration-ms` |
| Click point result | `action click --help` lists `--report-point` |
| Explicit task state | `agent state --help` lists state/run/Target/version flags |
| Session pagination | `session list --help` lists `--page` and `--page-size` |
| Local Moli | Top-level `--help` lists `--provider` with `moli` |

The bootstrap scripts retain their pinned release. If help rejects a command or
omits its flag, do not infer support from a newer Skill or rebuild/repackage a
different binary into the installed Skill. Use a compatible binary explicitly
provided for the task, or report the release dependency. Keep the intended
provider and exact Target when doing so.

## Cloud commands

```text
browser-cli doctor
browser-cli auth status
browser-cli auth login [--client-name "NAME"] [--connect-base-url URL]

browser-cli session create [--browser-mode normal|light]
  [--context-id ID --context-mode read_write|read_only]
  [--downloads] [--recording] [--window-size 1920,1080]
  [--official-proxy | --local-proxy-tunnel-id TUNNEL_ID]
browser-cli tunnel open [--duration SECONDS]
browser-cli tunnel status --tunnel-id TUNNEL_ID
browser-cli tunnel close --tunnel-id TUNNEL_ID

browser-cli session list [--status active] [--page 1] [--page-size 100]
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
browser-cli action --target-id PAGE_ID click --session-id ID --selector CSS --report-point
browser-cli action --target-id PAGE_ID move --session-id ID --selector CSS
  --from-json ACTUAL_PREVIOUS_DATA_JSON [--duration-ms 600]
browser-cli action fill --session-id ID --selector CSS --value TEXT
browser-cli action screenshot --session-id ID --path FILE [--full-page]
browser-cli action pdf --session-id ID --path FILE [--print-background]
browser-cli action eval --session-id ID --expression JS
browser-cli action raw --session-id ID --method CDP_METHOD --params-json JSON
```

## Page selection

Introduced in 1.2.0; requires a binary whose `browser-cli action --help` lists
`--target-id`. Earlier releases do not have this feature.
Check the actual Skill-local binary, not just the version of these instructions.

For an authorized upgrade, rerun the matching Skill-local bootstrap script only
after its pinned release assets are available, then verify `version` and
`action --help`. A merged PR or a newer Skill file does not publish or replace
the binary. If the release is unavailable or the upgrade is not authorized,
report the dependency or capability limitation; do not send unsupported flags
or drop the target selection to continue against a different page.

Every `action` above accepts optional `--target-id PAGE_ID`, before or after the
action subcommand. It selects an existing page inside `--session-id`; it is not
a replacement for the browser session ID. `move` requires an explicit Target.
Ordinary action result shapes remain unchanged; `click --report-point` is an
opt-in point object.

```text
browser-cli session targets --session-id SESSION_ID
browser-cli action snapshot --session-id SESSION_ID --target-id PAGE_ID
browser-cli action click --session-id SESSION_ID --target-id PAGE_ID --selector CSS
browser-cli session targets --session-id SESSION_ID
browser-cli action wait-selector --session-id SESSION_ID --target-id NEW_PAGE_ID --selector CSS
browser-cli action snapshot --session-id SESSION_ID --target-id NEW_PAGE_ID
```

Use the page entry's `id` in the DevTools target listing (`targetId` when using
CDP `Target.getTargets` directly). Match the expected URL/title and page type,
not the first/last position or an attached CDP `sessionId`. A popup may take time
to appear or navigate: refresh the listing within a bounded task timeout, then
wait for the required selector/text on the selected page. If several pages are
plausible, inspect them before choosing; do not blindly retry a state-changing
click. Pass the chosen ID on each subsequent action; selection is not persisted.

Without this option, the CLI keeps its original default: attach to the first
page returned by CDP, or create `about:blank` if no page exists. It does not
automatically follow a newly opened tab. A missing/closed explicit target fails
with `not_found`; a non-page target fails with `configuration_error`. A target
closed during attachment can produce `cdp_error`. The CLI never falls back to
another page or creates a page when an explicit target cannot be used. Re-list
targets and reassess the task instead of dropping `--target-id` to bypass errors.

Use temporary sessions for public browsing. Cloud login reuse uses a dedicated
persistent Context per account or purpose; avoid sharing one read-write Context
between parallel tasks. Cloud `--browser-mode light` remains a cloud Session,
not the local Moli provider or a local profile manager.

`wait-text` uses case-insensitive normalized contains matching by default. Add
`--exact` only when the entire normalized text must match. The obsolete
`--match contains` form is not supported.

## Real mouse movement

After checking the relevant help, an already-required authorized click can opt
into its actual endpoint. Ordinary `click` still returns `data:true`.

```text
browser-cli action --target-id PAGE_ID click --session-id SESSION_ID --selector CSS --report-point
browser-cli action --target-id PAGE_ID move --session-id SESSION_ID --selector DESTINATION_CSS --from-json ACTUAL_PREVIOUS_DATA_JSON --duration-ms 600
```

`ACTUAL_PREVIOUS_DATA_JSON` means the complete `data` object from that real click
or the preceding successful move, encoded as one JSON argument. It includes
`x`, `y`, `target_id`, `coordinate_space:"css_viewport"`, `viewport_width`,
`viewport_height`, `device_scale`, `page_scale` and `page_generation`. A move adds
`steps`, `elapsed_ms` and `requested_duration_ms`; its endpoint can seed the next
move. Do not fill unknown metadata, invent a start point, or click an unrelated
element just to get a cursor origin. Without a valid real origin, omit the
optional movement and use the task's normal input commands.

The origin must still match the exact Target, document and full viewport
metadata. Navigation, resizing, external pointer input or a changed page can
invalidate it; do not reuse stale coordinates after an error. Duration defaults
to **600 ms**, with accepted values 100–2000 ms. The accepted 800 ms demonstration
was an explicit override, not a CLI default.

Movement sends real unpressed CDP samples to the locked original DOM object and
endpoint, with monotonic timing and eased geometry. Each sample waits for CDP;
latency can reduce sample count and extend elapsed time. A changed document,
viewport, covered/disabled/detached element or moved geometry stops the command.
It does not retry, chase a replacement selector, press a button or switch Tabs.
Hover handlers may still have real page effects. CDP input ACK confirms dispatch,
not that the site's higher-level task succeeded; verify the resulting page state.

Desktop pointer observations are emitted after successful CDP responses, and
their local delivery remains best effort. A compatible desktop viewer retains
the last real pointer while idle or awaiting handoff, only while Target,
document, viewport/scale and connection remain valid. Click feedback fades
separately; do not generate fake motion or repeat input to keep it visible.

## Desktop Agent state

With actual `agent state --help` support, use one opaque `RUN_ID` for a logical
task and increase `--state-version` for each report within that Session/run,
including when the task changes Target.

```text
browser-cli agent state --session-id SESSION_ID --target-id PAGE_ID --run-id RUN_ID --state running --state-version 1
browser-cli agent state --session-id SESSION_ID --target-id PAGE_ID --run-id RUN_ID --state needs_human --state-version 2 --handoff-id HANDOFF_ID
# Only after the task's actual goal is verified:
browser-cli agent state --session-id SESSION_ID --target-id PAGE_ID --run-id RUN_ID --state completed --state-version 3
```

States are `running`, `needs_human`, `completed` and `failed`. Version must be
positive. `--handoff-id` is required only for `needs_human` and rejected for the
other states. Keep the ID for the same pending handoff; use a new ID only for a
new human intervention, rather than repeatedly triggering notification effects.
Use the real Session/Target IDs; task reporting itself does not verify that the
page exists. Stop issuing browser input before waiting for the user. These
reports do not pause/resume the Agent, host it remotely, close a Session or
complete a server task. A finished CLI command is not task completion.

The result contains `event_kind:"task.state"`, `attempted`,
`delivery:"best_effort"` and `server_persisted:false`. Even `attempted:true`
means only a local send attempt, not desktop acceptance/display, persistence or
an end-to-end ACK. Missing/disconnected local observation channels do not change
the real browser command result and must not cause input retries. The client
normally discovers the same-user app connection; `LEXMOUNT_APP_EVENTS=off`
disables observations. Do not expose its IPC token or synthesize raw events.

For Moli, prefix these commands with `--provider moli`; the instance reference
is separate from cloud project/account identity.

## Session pagination

`session list` reads **one page**, not all active Sessions. With supported help,
use positive `--page` (1-based) and `--page-size` values:

```text
browser-cli session list --status active --page 1 --page-size 100
browser-cli session list --status active --page 2 --page-size 100
```

Read `data.sessions` and `data.pagination.currentPage`/`totalPages` to request
further pages within the task's bounded discovery. Omitted flags retain server
defaults; `100` above is an example, not an imposed default. The CLI does not
automatically combine pages. If the binary lacks pagination flags, do not claim
that its one returned page is the complete list. Supported Moli lists use the
same page flags and response structure for the owned local registry.

## Local Moli

The `moli` provider manages CLI-created local instances, not cloud light mode or
arbitrary existing Chrome/Safari sessions. It needs no cloud account and does
not load cloud credentials. The owned-process implementation is for macOS and
Linux; Windows is not implemented. The current integration baseline is official
Moli v1.1.18, with Mac validation; method-by-method CDP compatibility still
matters, so shared action syntax does not prove every Chromium feature works.

```text
browser-cli --provider moli session create
browser-cli --provider moli session list --status active --page 1 --page-size 100
browser-cli --provider moli session get --session-id MOLI_SESSION_ID
browser-cli --provider moli session targets --session-id MOLI_SESSION_ID
browser-cli --provider moli action --target-id PAGE_ID snapshot --session-id MOLI_SESSION_ID
browser-cli --provider moli session close --session-id MOLI_SESSION_ID
```

Check actual top-level provider help first, then retain the provider on every
command. Moli supports create/list/get/targets/close and existing actions whose
protocol methods it implements. Cloud Contexts, profile options, tunnels,
downloads management, keepalive and cloud-specific create options are rejected.
Do not use cloud auth/doctor to fix a local executable, registry or loopback
failure, or silently switch to cloud when the local operation fails.

Executable discovery checks an explicit absolute `LEXMOUNT_MOLI_BINARY`
development override first, then bundled `moli` beside the CLI, the local
`~/.local/share/lexmount/moli/moli`, then absolute PATH directories. A bad
explicit override fails. Bundled lookup support does not mean the installed
desktop package or old Skill release already contains Moli; report a missing
dependency instead of promising installation.

The same-user private registry records only CLI-owned process identities and
loopback endpoints. Instance IDs are opaque `moli_…` values. `session targets`
returns page IDs just as cloud does; do not infer Moli identity from a reported
Chromium version string. Explicit close applies only to the verified owned
instance. A viewer disconnect or return-to-overview does not close it. Check
the close result; a failed or uncertain close is not proof that the process
stopped. Do not resolve it by killing an unrelated PID or attaching elsewhere.
There is no local persistent-profile or normal-close save guarantee.

## Local network proxy (1.3.0)

Check that the installed binary lists `tunnel` and
`session create --local-proxy-tunnel-id`. Run `tunnel open` on the machine whose
local/VPN network is needed, keep that process running, and read its single
flushed JSON ready result. In a separate invocation create a normal session with
that tunnel ID, then use existing actions. The tunnel ID pins the region; use
the same project and API environment. It conflicts with `--official-proxy` and
a different `LEXMOUNT_REGION`. The server requires `custom_proxy` capability.

This grants selected cloud browser sessions access through that machine's local
DNS and TCP connections. It does not copy cookies or OS login, or tunnel UDP.
Do not start a local connector on a different machine from the intended network.
There is no cloud/direct fallback on tunnel failure. Reconnection terminates old
TCP streams; it never replays requests or browser actions. `tunnel status` checks
current server state; the initial ready output is not a continuous status feed.

Close browser sessions when done, then stop the foreground connector with Ctrl+C
(or `tunnel close --tunnel-id ID`). Closing the tunnel does not close sessions or
stop session billing. `--duration` limits tunnel lifetime and defaults to 0
(until interrupted or revoked). This process must remain alive between CLI
invocations, and does not keep browser sessions active by itself. Closing stdin
is not the shutdown signal; use Ctrl+C/termination or the close command.
