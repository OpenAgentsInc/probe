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
composer now submits through the real Probe runtime, using the backend resolved
from `~/.probe/server/psionic-local.json` together with the
`coding_bootstrap` harness and conservative tool approval policy. The first
submit creates a persisted Probe session; later submits continue that same
session. The active-turn cell is now driven by real runtime lifecycle events,
so the TUI can show model requests, tool request/start/completion, refusal or
pause, and the final assistant commit before the transcript delta is rendered.
Persisted tool activity renders as first-class transcript rows such as
`[tool call]`, `[tool result]`, and `[approval pending]` rather than generic
notes.

Setup, help, and approval flows live in a typed overlay stack above or in
place of the composer. A background Apple FM availability/setup check still
runs on launch, and the full setup detail can be opened on demand from the
overlay.

Views:

- `Chat`: transcript-first home shell with the live composer
- `Events`: shell and worker event logs

Keys:

- `Tab`, `Shift+Tab`: switch `Chat` / `Events`
- `Enter`: submit the composer
- `Ctrl+J`: insert a newline
- `Up`, `Down`: recall draft history
- `Ctrl+O`: add an attachment placeholder to the draft
- `Ctrl+R`: rerun setup
- `Ctrl+S`: open setup overlay
- `Ctrl+A`: open approval overlay
- `Ctrl+T`: toggle operator notes vs live detail
- `F1`: help
- `Esc`: dismiss modal
- `Ctrl+C`: quit

The composer is active on `Chat` and disabled while overlays own focus or while
`Events` is selected. The draft model tracks slash commands, typed mentions
such as `@skill:rust` or `@app:github`, attachment placeholders, submission
history, and multiline paste state. When a tool pauses for approval, Probe now
persists a real pending-approval record in `probe-core`, opens the approval
overlay with the live tool details, and resumes the paused turn after approve
or reject instead of leaving the operator in a dead-end pause state.

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
