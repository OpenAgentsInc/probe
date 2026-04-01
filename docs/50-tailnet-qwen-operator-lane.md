# Tailnet-Hosted Psionic Qwen Operator Lane

## Why

Probe already knew how to attach to a remote OpenAI-compatible backend, but
that path was still implicit:

- `probe chat` prepared the backend target through the saved server config
- `cargo probe` only reconstructed a profile from saved config and skipped the
  full CLI readiness contract
- the TUI still foregrounded local Apple FM startup even when the active chat
  backend was Qwen
- the operator had no explicit TUI or CLI surface for `this session is using a
  remote Tailnet or SSH-forwarded Psionic server`

## What Changed

- `crates/probe-core/src/server_control.rs`
  - adds a typed `ServerOperatorSummary`
  - classifies prepared targets as:
    - `managed_launch`
    - `loopback_or_ssh_forward`
    - `tailnet_attach`
    - `remote_attach`
- `crates/probe-cli/src/main.rs`
  - `probe tui` now accepts `--profile`, `--probe-home`, `--cwd`, and the same
    flattened server args as `probe chat`
  - `probe tui` now runs `prepare_server(...)` before launching the UI and keeps
    the resulting guard alive for the TUI lifetime
  - CLI surfaces now print a concise backend target summary including backend
    kind, attach mode, transport, host or port, model, and base URL
- `crates/probe-tui/src/app.rs`
  - adds a typed TUI launch config carrying the prepared runtime profile plus
    operator backend summary
  - default startup only auto-runs Apple FM setup when the active chat backend
    is actually Apple FM
- `crates/probe-tui/src/screens.rs`
  - stores and renders the operator backend summary
  - upgrades the old setup overlay into a backend overlay
  - shows the remote operator contract explicitly for non-Apple-FM lanes
  - keeps the compact backend summary visible in the bottom status bar

## Operator Contract

The first remote Probe lane stays narrow:

- local Probe owns sessions, transcripts, tools, approvals, and UI
- remote Psionic serves inference only
- attach can be direct Tailnet IP or SSH-forwarded localhost
- Tailnet or SSH forwarding remains the first real security boundary
- remote runtime orchestration is out of scope here

## Practical Paths

### SSH-forwarded localhost

- keep the remote Psionic Qwen server running on the Tailnet node
- forward a local port over SSH
- point Probe at the loopback target with `--server-host 127.0.0.1 --server-port ...`
- Probe labels that lane as `loopback_or_ssh_forward`

### Direct Tailnet attach

- point Probe at the Tailnet IP directly with `--server-host 100.x.y.z`
- Probe labels that lane as `tailnet_attach`

## Shipped UX

- `probe chat` and `probe exec` now print the prepared backend target summary
- `probe tui` uses the same prepared backend contract instead of hidden config
  reconstruction
- the TUI bottom status bar shows backend kind, target, attach mode, model, and
  live phase
- `Ctrl+S` opens a backend overlay that shows the operator contract for remote
  Qwen lanes instead of unrelated Apple FM setup detail
- Apple FM startup checks only foreground when the active chat backend is Apple
  FM

## Validation

- `cargo test -p probe-core --lib -- --nocapture`
- `INSTA_UPDATE=always cargo test -p probe-tui -- --nocapture`
- `INSTA_UPDATE=always cargo test -p probe-cli --test cli_regressions -- --nocapture`
- `cargo check --workspace`
