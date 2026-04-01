# AGENTS

This file defines the shared agent contract for the `probe` repo.

## Purpose

`probe` owns the coding-agent runtime itself.

That includes:

- runtime session lifecycle
- protocol and event model
- tool execution
- permission and approval policy
- persistence and recovery
- CLI and server surfaces

This repo should not quietly absorb unrelated product-shell concerns.

## Working Rules

- Keep the runtime boundary explicit.
- Prefer a small set of focused crates over one oversized core crate.
- Treat the machine-readable protocol as a first-class product surface.
- Keep session, turn, item, and task models explicit in code and docs.
- Separate execution policy from sandbox and executor mechanics.
- Prefer append-only runtime artifacts plus indexed metadata over opaque blobs.

## Early Priorities

- establish a Rust workspace
- define the protocol types
- build the first local server mode
- build the CLI on the same client/runtime contract
- add durable session storage
- add the first typed tool and approval flows

## Start Here

- `README.md`
- `docs/51-probe-optimizer-system.md`
- `docs/02-runtime-ownership-and-boundaries.md`
- `docs/03-workspace-map.md`
- `docs/24-apple-fm-backend-lane.md`
- `docs/25-apple-fm-tool-lane.md`
- `docs/31-probe-tui-background-task-and-app-message-bridge.md`
- `docs/32-apple-fm-setup-screen.md`
- `docs/35-probe-tui-retained-transcript-model.md`
- `docs/36-chat-screen-primary-shell-and-setup-secondary.md`
- `docs/37-probe-tui-bottom-pane-and-minimal-composer.md`
- `docs/38-probe-tui-transcript-turn-rendering.md`
- `docs/39-probe-tui-typed-overlay-stack-and-focus-routing.md`
- `docs/40-probe-tui-composer-history-commands-mentions-attachments-and-paste.md`
- `docs/42-probe-tui-real-runtime-session-worker.md`
- `docs/43-probe-runtime-event-stream-and-live-tui-lifecycle.md`
- `docs/44-probe-tui-tool-call-and-tool-result-rows.md`
- `docs/45-probe-tui-resumable-approval-broker.md`
- `docs/47-openai-streaming-runtime-delta-events.md`
- `docs/48-apple-fm-streaming-and-snapshot-events.md`
- `docs/49-probe-tui-streamed-output-rendering.md`
- `docs/50-tailnet-qwen-operator-lane.md`

Add more canonical docs here as the repo grows.
