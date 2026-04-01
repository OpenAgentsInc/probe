![Protoss Probe](assets/images/protossprobe.jpg)

# Probe

Probe is a Rust-first coding-agent runtime. It owns session lifecycle,
transcript persistence, tool execution, approvals, backend attachment, and the
CLI/TUI surfaces above that runtime.

Current shipped surface:

- `probe exec` for one-shot turns
- `probe chat` for interactive sessions plus resume
- `probe tui` / `cargo probe` for the local terminal UI
- `coding_bootstrap` tools, approvals, and harness profiles
- append-only local transcripts under `PROBE_HOME` or `~/.probe`
- bounded oracle and long-context escalation lanes
- local acceptance/eval and module-optimization tooling

## Backends

Probe currently ships two backend families:

- `psionic-qwen35-2b-q8-registry`
  - base URL: `http://127.0.0.1:8080/v1`
  - model: `qwen3.5-2b-q8_0-registry.gguf`
- `psionic-apple-fm-bridge`
  - default base URL: `http://127.0.0.1:11435`
  - model: `apple-foundation-model`
  - override order: `PROBE_APPLE_FM_BASE_URL`, then `OPENAGENTS_APPLE_FM_BASE_URL`

Apple FM is attach-only. Probe checks `GET /health` before use and stays honest
about unavailable or non-admitted machines.

## Quick Start

Run the TUI:

```bash
cargo probe
```

Run a one-shot turn:

```bash
cargo run -p probe-cli -- exec "Explain what this repository does."
```

Start an interactive session:

```bash
cargo run -p probe-cli -- chat
```

Resume a session:

```bash
cargo run -p probe-cli -- chat --resume <session-id>
```

Run a tool-enabled turn:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --harness-profile coding_bootstrap_default \
  --tool-choice auto \
  "Read README.md and summarize what this repository does."
```

## TUI

`cargo probe` is the current top-level Probe UI entrypoint. The current shell
uses a retained transcript widget with committed user, tool, and assistant
turns plus one explicit active-turn cell. `Chat` is the home surface and the
composer now submits through the real Probe runtime. `probe tui` now uses the
same prepared backend contract as `probe chat`: it resolves the selected
backend, runs server readiness or attach preparation first, and then carries
the prepared host, port, model, backend kind, and attach mode into the UI.
The first submit creates a persisted Probe session; later submits continue that
same session. The active-turn cell is now driven by real runtime lifecycle
events, so the TUI can show model requests, tool request/start/completion,
refusal or pause, and the final assistant commit before the transcript delta
is rendered. Persisted tool activity renders as first-class transcript rows
such as `[tool call]`, `[tool result]`, and `[approval pending]` rather than
generic notes.

At the runtime layer, Probe now distinguishes backend streaming truth
explicitly: OpenAI-compatible backends stream assistant deltas, while Apple FM
streams full session snapshots rather than fake token deltas. The chat surface
now renders those streams honestly in place: one retained active cell grows
with streamed deltas or snapshot replacement until the authoritative
transcript rows land, streamed tool-call assembly stays visible before final
tool rows commit, and the bottom status bar carries a compact backend and
stream summary.

Setup, help, and approval flows live in a typed overlay stack above or in
place of the composer. The old setup surface is now a backend overlay:
Apple FM launches still foreground local Apple FM admission and setup truth,
while Qwen launches show the prepared backend target and the remote operator
contract instead of unrelated Apple FM chrome.

The first supported remote-Qwen posture stays narrow and explicit:

- local Probe owns sessions, transcripts, tools, approvals, and UI
- remote Psionic serves inference only
- `127.0.0.1` attach is treated as local or SSH-forwarded
- `100.x.y.z` attach is treated as direct Tailnet attach

The top selector is now a backend switcher, not a view switcher. Probe keeps one
transcript-first home shell on screen at a time, and `Tab` flips the active
runtime between the current Qwen or Tailnet lane and the Apple FM lane. When
the backend changes, Probe resets the chat surface so the next submit starts a
fresh session on that backend instead of continuing the previous runtime lane.
Each backend lane now keeps its own saved attach config under
`~/.probe/server/psionic-openai-chat-completions.json` or
`~/.probe/server/psionic-apple-fm.json`, so switching back does not fabricate a
fresh localhost target.

Keys:

- `Tab`, `Shift+Tab`: switch active backend
- `Enter`: submit the composer
- `Ctrl+J`: insert a newline
- `Up`, `Down`: recall draft history
- `Ctrl+O`: add an attachment placeholder to the draft
- `Ctrl+R`: rerun backend check when supported
- `Ctrl+S`: open backend overlay
- `Ctrl+A`: open approval overlay
- `Ctrl+T`: toggle operator notes vs live detail
- `F1`: help
- `Esc`: dismiss modal
- `Ctrl+C`: quit

The composer is active on the main transcript shell and disabled while overlays
own focus. The draft model tracks slash commands, typed mentions such as
`@skill:rust` or `@app:github`, attachment placeholders, submission history,
and multiline paste state. When a tool pauses for approval, Probe now persists
a real pending-approval record in `probe-core`, opens the approval overlay with
the live tool details, and resumes the paused turn after approve or reject
instead of leaving the operator in a dead-end pause state.

Remote attach examples:

```bash
# direct Tailnet attach
cargo run -p probe-cli -- tui \
  --profile psionic-qwen35-2b-q8-registry \
  --server-host 100.88.7.9 \
  --server-port 8080

# SSH-forwarded localhost attach
cargo run -p probe-cli -- tui \
  --profile psionic-qwen35-2b-q8-registry \
  --server-host 127.0.0.1 \
  --server-port 8080
```

## Dev Helpers

Repo-local helpers:

```bash
./probe-dev fmt
./probe-dev check
./probe-dev test
./probe-dev accept
```

Useful focused lanes:

- `./probe-dev pr-fast`
- `./probe-dev cli-regressions`
- `./probe-dev accept-live`
- `./probe-dev accept-compare`

## Docs

Start with:

- [docs/02-runtime-ownership-and-boundaries.md](docs/02-runtime-ownership-and-boundaries.md)
- [docs/03-workspace-map.md](docs/03-workspace-map.md)
- [docs/24-apple-fm-backend-lane.md](docs/24-apple-fm-backend-lane.md)
- [docs/25-apple-fm-tool-lane.md](docs/25-apple-fm-tool-lane.md)
- [docs/31-probe-tui-background-task-and-app-message-bridge.md](docs/31-probe-tui-background-task-and-app-message-bridge.md)
- [docs/32-apple-fm-setup-screen.md](docs/32-apple-fm-setup-screen.md)
- [docs/35-probe-tui-retained-transcript-model.md](docs/35-probe-tui-retained-transcript-model.md)
- [docs/36-chat-screen-primary-shell-and-setup-secondary.md](docs/36-chat-screen-primary-shell-and-setup-secondary.md)
- [docs/37-probe-tui-bottom-pane-and-minimal-composer.md](docs/37-probe-tui-bottom-pane-and-minimal-composer.md)
- [docs/38-probe-tui-transcript-turn-rendering.md](docs/38-probe-tui-transcript-turn-rendering.md)
- [docs/39-probe-tui-typed-overlay-stack-and-focus-routing.md](docs/39-probe-tui-typed-overlay-stack-and-focus-routing.md)
- [docs/40-probe-tui-composer-history-commands-mentions-attachments-and-paste.md](docs/40-probe-tui-composer-history-commands-mentions-attachments-and-paste.md)
- [docs/42-probe-tui-real-runtime-session-worker.md](docs/42-probe-tui-real-runtime-session-worker.md)
- [docs/43-probe-runtime-event-stream-and-live-tui-lifecycle.md](docs/43-probe-runtime-event-stream-and-live-tui-lifecycle.md)
- [docs/44-probe-tui-tool-call-and-tool-result-rows.md](docs/44-probe-tui-tool-call-and-tool-result-rows.md)
- [docs/45-probe-tui-resumable-approval-broker.md](docs/45-probe-tui-resumable-approval-broker.md)
- [docs/47-openai-streaming-runtime-delta-events.md](docs/47-openai-streaming-runtime-delta-events.md)
- [docs/48-apple-fm-streaming-and-snapshot-events.md](docs/48-apple-fm-streaming-and-snapshot-events.md)
- [docs/49-probe-tui-streamed-output-rendering.md](docs/49-probe-tui-streamed-output-rendering.md)
- [docs/50-tailnet-qwen-operator-lane.md](docs/50-tailnet-qwen-operator-lane.md)
