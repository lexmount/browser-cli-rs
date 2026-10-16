# Optional desktop event channel (schema 1)

The CLI remains the execution interface. This channel only observes an action;
the desktop app is not a browser-command gateway. The existing single JSON
result on stdout, JSON failure on stderr and exit status remain unchanged.

## Opt in

Normally the CLI discovers the running app through its private file
`~/.local/state/lexmount/app-connection.json` (override with the absolute path
in `LEXMOUNT_APP_CONNECTION_FILE`). The file contains:

```json
{"schema_version":1,"endpoint":"/tmp/private-user-dir/events.sock","token":"<IPC token>","app_instance_id":"<app launch identity>"}
```

On Unix, both the file and its direct parent must be owned by the effective
current user, with exact permissions 0600 and 0700 respectively. File/parent
symlinks are rejected; the file is opened without following symlinks and
validated again after opening, with a 16 KiB size bound. Missing, malformed,
insecure or unsupported files silently disable events. The app creates this
file for its lifetime and removes it when it exits.

Alternatively set these environment variables in the Agent's CLI process:

- `LEXMOUNT_APP_EVENT_ENDPOINT`: absolute Unix domain socket pathname.
- `LEXMOUNT_APP_EVENT_TOKEN`: independent local IPC token issued by the app.
  This is **not** the Lexmount API key. Do not log it or pass it in CLI arguments.
- `LEXMOUNT_AGENT_RUN_ID`: optional opaque ID, up to 128 UTF-8 bytes, provided
  by the Agent integration to correlate commands from the same run.

Explicit endpoint/token take priority as a pair: if only one is set, events are
disabled instead of mixing environment and discovered credentials. Set
`LEXMOUNT_APP_EVENTS=off` to disable all event delivery, including discovery;
the app's own screenshot/polling processes must set this explicitly.

Missing or invalid endpoint/token disables observations. Mac and Linux use a
nonblocking Unix stream socket. **Windows transport is not implemented yet**;
the explicit platform adapter drops events until a same-user, nonblocking named
pipe implementation is added. There is no TCP fallback on any platform.

The app owns endpoint creation, access permissions and token lifetime. Put the
socket in a private same-user directory; do not place the token in a repository
or a world-readable file. The CLI does not start a daemon or change its
credential store. The discovery file contains no Lexmount API key.

## Framing

Each event opens a fresh connection, writes one UTF-8 JSON line and closes it.
The entire frame, including the newline, is limited to 16 KiB. The envelope is:

```json
{
  "token": "<independent IPC token>",
  "event": {
    "schema_version": 1,
    "event_id": "<32 lowercase hex characters>",
    "producer_id": "<random process identity>",
    "operation_id": "<random action invocation identity>",
    "seq": 1,
    "emitted_at": 1792112400123,
    "connection_ref": "lexmount:<64 lowercase hex characters>",
    "session_id": "<cloud Session ID>",
    "target_id": null,
    "run_id": null,
    "app_instance_id": null,
    "kind": "command.started",
    "payload": {"action": "click"}
  }
}
```

`app_instance_id` is copied from the authenticated private discovery file, or
null for an explicit environment pair. `emitted_at` is Unix epoch milliseconds.
`seq` increases within an operation;
gaps are valid because delivery is best effort. `producer_id` is stable only for
the current CLI process; `operation_id` is new for every action invocation.

`connection_ref` is `lexmount:` followed by the lowercase SHA-256 digest of:
`URL origin as serialized by the url crate` + one zero byte + `project ID`.
For example the normalized origin is `https://api.example.test` for
`https://api.example.test:443/path`. The digest excludes path, query, userinfo
and API key. The project ID itself does not appear in the envelope.

The receiver must authenticate the token before accepting the event; bound the
input before JSON parsing, reject missing-newline/truncated frames and unknown
schemas, deduplicate event IDs, and discard stale/unknown Session or Target IDs.
It must not log the token or expose a native-command bridge to a web preview.

## Event order and meaning

1. `command.started`: sent before Session lookup; `target_id` is unknown/null.
2. `command.target`: sent after CDP attached and enabled the actual page target.
   The top-level `target_id` now identifies that exact page. It is not copied
   from the CLI's requested `--target-id` flag. Payload is `{}`.
3. `pointer.move`, `pointer.down`, `pointer.up`: selector-click input that was
   actually sent to the CDP socket, before its acknowledgement.
4. `command.finished`: payload includes `action`, `outcome` (`success` or
   `error`) and `error_kind` (null on success). It deliberately omits error
   messages. Failure before CDP connection leaves `target_id` null.

An action's completion is **not** an Agent run's completion. Action events do
not claim that an Agent is done, paused, waiting for handoff, or that a website
accepted the input. Do not infer those states from a command's exit status or a
Session's `active` status. App-invoked screenshots/polling should disable this
Agent channel with `LEXMOUNT_APP_EVENTS=off`.

## Explicit Agent task state

The Agent can report its actual task state using:

```sh
browser-cli agent state --session-id SESSION --target-id TARGET --run-id RUN --state running --state-version 1
browser-cli agent state --session-id SESSION --target-id TARGET --run-id RUN --state needs_human --state-version 2 --handoff-id HANDOFF
browser-cli agent state --session-id SESSION --target-id TARGET --run-id RUN --state running --state-version 3
browser-cli agent state --session-id SESSION --target-id TARGET --run-id RUN --state completed --state-version 4
```

States are `running`, `needs_human`, `completed`, or `failed`. All require an
exact Session ID, Target ID, explicit run ID and positive state version.
`needs_human` requires a handoff ID; the other states reject one. Run/handoff IDs
are nonempty opaque strings up to 128 bytes, without control characters.

The Agent integration owns the stable run ID and monotonically increasing
`state_version` for that run. Repeated reports for the same handoff retain their
handoff ID, enabling notification deduplication. The explicit `--run-id` takes
priority over `LEXMOUNT_AGENT_RUN_ID`. This command reports known state; it does
not decide when an Agent needs a human or mark tasks complete automatically.

During a long-running task, report `running` at least every 30 seconds with a
higher `state_version`, even when no browser input is needed. The desktop keeps
the explicit task phase but shows “状态待确认” after 60 seconds without a fresh
running report or active command. This is a freshness indicator, not task
completion, cancellation or an automatic handoff. Waiting-for-human state stays
visible until the Agent explicitly changes it.

Each report emits exactly one `task.state` event, with a new operation ID and
`seq=1`. The top-level Target/run IDs come from the explicit arguments, and
payload is `{"state":"needs_human","state_version":2,"handoff_id":"HANDOFF"}`.
The receiver must verify that the reported Session/Target exists in its current
connection and reject old state versions. No CDP lookup or cloud API request is
made; credentials are read only to select the same connection namespace.

The JSON command result includes `event_kind:"task.state"`, `delivery:
"best_effort"`, `attempted:true|false` and `server_persisted:false`.
`attempted` only says that a valid local channel was configured and a send was
attempted. It does not mean the app received/accepted the event. A missing or
disabled channel returns `attempted:false`; a stopped app does not turn this
report into a browser or Agent-task failure. The command never persists,
executes, pauses or completes a cloud task.

Pointer payloads contain:

```json
{
  "phase": "down",
  "x": 100.0,
  "y": 80.0,
  "coordinate_space": "css_viewport",
  "button": "left",
  "buttons": 1,
  "click_count": 1,
  "viewport_width": 1280.0,
  "viewport_height": 720.0,
  "device_scale": 2.0,
  "page_scale": 1.0,
  "page_generation": "1792112400000.25"
}
```

Coordinates are CSS viewport pixels resolved by the existing click operation.
The same DOM call returns `window.innerWidth/innerHeight`, device pixel ratio,
visual viewport scale, and `performance.timeOrigin` as a document generation.
Missing or invalid metadata is `null`, never a guessed size or generation.
The app must map through the displayed page's content rectangle, exclude
letterboxing/toolbars and reject a mismatched Target/document generation.
Unknown viewport metadata must not render a falsely positioned cursor.

Only the existing selector `click` implementation produces pointer events in
this version. Raw CDP input, JavaScript `eval`, fill, scrolling or browser actions
performed by another tool do not synthesize pointer observations. No selectors,
typed text, expressions, page URLs, WebSocket URLs or API keys enter events.

## Delivery failures

Connect and write are nonblocking from their first syscall. A stopped app, full
accept backlog, nonreading collector, short/partial write or invalid socket path
silently drops that observation. The CLI does not wait for an acknowledgement or
retry the browser operation. The collector must discard an incomplete frame
when the connection closes. Delivery is not a durable event log and only covers
CLI processes on this machine configured with this endpoint and token.
