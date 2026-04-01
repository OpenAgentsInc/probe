# Probe Docs

This folder holds technical planning docs for the Probe runtime.

## Table Of Contents

- `01-psionic-qwen-hermes-deep-dive-and-probe-cli-roadmap.md`
  - deep dive on the prior Psionic Hermes/Qwen work and the concrete roadmap
    for consuming that backend from the first Rust Probe CLI
- `02-runtime-ownership-and-boundaries.md`
  - ownership line for what Probe should own itself and what it should consume
    from the backend substrate
- `03-workspace-map.md`
  - initial crate map for the Probe Rust workspace
- `04-session-turn-item-and-transcript-model.md`
  - the first durable local truth model for sessions, turns, items, and
    append-only transcript storage
- `05-openai-compatible-provider-client.md`
  - the first typed backend client seam for local OpenAI-compatible endpoints
- `06-psionic-qwen35-backend-profile.md`
  - the first explicit built-in backend profile for a local Psionic-served
    Qwen3.5 model
- `07-probe-exec.md`
  - the first non-interactive end-to-end Probe controller lane with local
    transcript persistence
- `08-interactive-cli-and-resume.md`
  - the first interactive session loop and transcript-backed resume flow
- `09-tool-loop-and-local-tools.md`
  - the bounded local coding tool runtime, batch execution path, and replay
    contract
- `10-acceptance-harness.md`
  - the retained local acceptance runner for plain and tool-backed controller
    cases
- `11-server-attach-and-launch.md`
  - local server config, attach mode, and supervised launch behavior for
    `psionic-openai-server`
- `12-observability-and-cache-signals.md`
  - per-turn wallclock, token usage, throughput, and cache-signal behavior for
    the first local controller lane
- `13-harness-profiles.md`
  - Probe-owned versioned harness profiles for the coding tool lane, including
    the first `coding_bootstrap_default@v1` profile and its relationship to
    `--system`
- `14-approval-classes-and-structured-tool-results.md`
  - explicit risk classes, local approval policy, CLI approval controls, and
    structured tool-result records for the coding tool lane
- `15-replay-and-decision-dataset-export.md`
  - local-first JSONL export for replay and derived decision datasets from
    real Probe sessions
- `16-decision-modules.md`
  - narrow Rust-native decision-module boundary above the runtime, plus the
    first offline-evaluable `ToolRoute` and `PatchReadiness` modules
- `17-offline-optimizer.md`
  - GEPA-style offline optimization receipts, shared promotion rules, and the
    baseline-versus-candidate comparison flow for modules and harness reports
- `18-oracle-consultation.md`
  - typed bounded oracle consultation as an auxiliary tool and backend role
- `19-long-context-repo-analysis.md`
  - opt-in bounded repo-analysis escalation with explicit evidence pointers,
    budgets, and transcript-visible provenance
- `20-testing-and-local-runner.md`
  - shared test-support helpers, canonical local validation commands, and the
    `nextest`-first runner contract
- `21-cli-regression-and-snapshots.md`
  - process-level binary tests, narrow snapshot coverage, and the normalized
    receipt boundary for the CLI surface
- `22-acceptance-report-schema.md`
  - run identity, backend and harness metadata, failure categories, counts,
    and transcript references for the richer `probe accept` report
- `23-local-test-tiers.md`
  - explicit local fast-test, binary-regression, live-acceptance, and
    offline-eval lanes in `probe-dev`
- `24-apple-fm-backend-lane.md`
  - the first real Apple FM backend lane for plain-text turns, server attach,
    and bounded oracle use
- `25-apple-fm-tool-lane.md`
  - session-backed Apple FM coding turns through Probe-owned tool callbacks,
    Probe transcript replay, and the existing approval or refusal policy
- `26-backend-receipts-and-usage-truth.md`
  - widened `u64` usage handling, exact-versus-estimated observability truth,
    and adjunct backend receipts such as Apple FM transcript exports or typed
    availability and refusal facts
- `27-apple-fm-qwen-comparison-suite.md`
  - admitted-Mac Apple FM acceptance runs, the retained overlapping case set,
    and the Probe-owned comparison artifact for Apple FM versus Psionic Qwen
- `28-admitted-mac-comparison-runbook.md`
  - the local admitted-Mac runbook for the heavy Apple FM versus Qwen
    comparison lane on self-hosted Apple hardware
- `30-textual-inspired-rust-tui-shell.md`
  - first Rust-native TUI bootstrap issue for a Textual-inspired Probe screen
    shell, proving basic app/screen/widget structure, keyboard-driven state
    changes, and a visible hello-world terminal UI target
- `31-probe-tui-background-task-and-app-message-bridge.md`
  - narrow retained worker thread and typed app-message seam for the Probe TUI,
    so screens can request bounded background work without freezing the render
    loop
- `32-apple-fm-setup-screen.md`
  - the first real Apple FM-backed Probe TUI setup surface, including
    availability gating, a retained startup setup check, and snapshot/test
    coverage for unavailable, running, and completed states
- `35-probe-tui-retained-transcript-model.md`
  - the explicit first transcript rendering decision for the Probe TUI:
    retained in-memory transcript widget plus one active-turn cell, before any
    Codex-style scrollback manager or full chat/composer shell
- `36-chat-screen-primary-shell-and-setup-secondary.md`
  - the TUI restructuring that makes `Chat` the primary Probe home surface and
    demotes Apple FM setup inspection into the secondary `Setup` tab
- `37-probe-tui-bottom-pane-and-minimal-composer.md`
  - the first real bottom-pane seam for Probe TUI input: a cursor-bearing
    composer, modifier-based global shell commands, explicit disabled/busy
    states, and bottom-pane-owned status rendering instead of a passive footer
- `38-probe-tui-transcript-turn-rendering.md`
  - the first real chat transcript turn model for Probe TUI, including visible
    user/tool/assistant entries, a worker-driven active-turn cell, and a more
    transcript-dominant shell layout
- `39-probe-tui-typed-overlay-stack-and-focus-routing.md`
  - the first typed overlay stack for Probe TUI, moving setup/help/approval
    flows out of the home layout and into explicit focused overlays that can
    disable or replace the composer
- `40-probe-tui-composer-history-commands-mentions-attachments-and-paste.md`
  - the first richer Probe draft model with shell-style history recall,
    slash-command and typed-mention semantics, attachment placeholders, and
    explicit paste-aware input handling
- `42-probe-tui-real-runtime-session-worker.md`
  - the first real `probe-core`-backed TUI turn loop, replacing the temporary
    worker with persisted runtime sessions, transcript hydration from the
    session store, and honest error recovery
- `43-probe-runtime-event-stream-and-live-tui-lifecycle.md`
  - typed per-turn runtime events in `probe-core`, plus the first live Probe
    TUI lifecycle rendering for model requests, tool activity, refusal/pause,
    and assistant commit
- `44-probe-tui-tool-call-and-tool-result-rows.md`
  - first-class transcript row kinds for persisted Probe tool calls, tool
    results, refusals, and approval-pending outcomes, plus compact operator
    summaries built from the stored `tool_execution` truth
- `45-probe-tui-resumable-approval-broker.md`
  - real pending-approval persistence, approve or reject resolution in
    `probe-core`, replay of resolved tool results into the paused turn, and
    TUI approval-overlay wiring against that runtime state
- `47-openai-streaming-runtime-delta-events.md`
  - streamed OpenAI-compatible SSE parsing in `probe-provider-openai`, runtime
    delta events for streamed assistant or tool-call progression, and JSON
    fallback for backends that still answer a streaming request with blocking
    chat-completion output
- `48-apple-fm-streaming-and-snapshot-events.md`
  - Apple FM session-response streaming in `probe-provider-apple-fm`, explicit
    snapshot semantics instead of fake token deltas, and runtime snapshot
    events that preserve the local Probe tool and approval contract
- `49-probe-tui-streamed-output-rendering.md`
  - real incremental TUI rendering for streamed OpenAI deltas and Apple FM
    snapshots, streamed tool-call assembly in the active cell, compact backend
    and stream state in the bottom status bar, and authoritative replacement by
    committed transcript rows
- `50-tailnet-qwen-operator-lane.md`
  - first-class remote-Qwen operator lane for `probe tui`, `probe chat`, and
    `probe exec`, including prepared backend summaries, backend-aware TUI
    startup, explicit Tailnet versus SSH-forwarded attach posture, and a
    backend overlay instead of local-Apple-FM-only setup chrome
- `51-probe-optimizer-system.md`
  - canonical end-to-end map of the offline optimizer subsystem, including
    retained source artifacts, manifest families, the Probe-to-Psionic bridge,
    promotion ledgers, adoption state, and current runtime boundaries
- `52-self-optimization-loop-and-long-horizon-learning-audit.md`
  - audit of what Probe still needs for a real self-optimization loop,
    including optimize-campaign runtime objects, long-horizon study lanes,
    richer export shapes, isolated candidate worktrees, stronger evals, and
    promotion-safe adoption stages
- `53-probe-background-agent-roadmap.md`
  - Probe-specific plan for becoming a real background coding agent with a
    server-first runtime, detached session workers, prepared workspaces, and
    CLI/TUI plus Autopilot as the first client set, updated after the shipped
    server, queue, Codex, and optimizer lanes so the next work starts from the
    real post-Phase-1 baseline
- `54-openai-codex-subscription-auth.md`
  - Probe-owned browser PKCE and headless device auth for ChatGPT/Codex
    subscriptions, including persisted token state, CLI commands, TUI backend
    overlay status, and the concrete reproduction flow for the shipped Codex
    subscription path
- `55-openai-codex-subscription-backend.md`
  - dedicated Codex subscription backend profile and transport, including the
    canonical `openai-codex-subscription` profile, request rewrite to
    `https://chatgpt.com/backend-api/codex/responses`, subscription header
    injection, model gating, and the CLI reproduction flow
- `56-probe-server-workspace-lifecycle-decisions.md`
  - pure workspace lifecycle state, timeout, restart, and circuit-breaker
    decisions for the serverized Probe path before detached workers or
    prepared baselines land
- `57-codex-third-inference-mode.md`
  - the third Probe TUI inference lane, backend selector order
    `Codex|Qwen|Apple FM`, Codex-specific prompt contracts for plain and
    tool-enabled turns, and the reproduction path for lane switching and
    hosted Codex execution
- `58-probe-server-stdio-runtime-protocol.md`
  - the first shipped `probe-server` contract: JSONL over stdio, explicit
    session and turn APIs, serializable tool-loop recipes, and typed
    best-effort versus lossless event classes for first-party clients
- `59-shared-test-support-and-stable-snapshot-root.md`
  - the completed shared test-support boundary for fake backends, temp Probe
    homes and workspaces, CLI launch helpers, stable snapshot-root setup, and
    shared report or transcript normalization utilities
- `60-probe-client-and-first-party-adoption.md`
  - shared `probe-client` spawn and transport ownership, the hidden internal
    server fallback for local development, and the first-party adoption path
    for `probe exec`, `probe chat`, and the TUI worker
- `61-probe-daemon-and-local-socket-transport.md`
  - the first detached-local transport layer for Phase 2, including the
    long-lived `probe-daemon`, Unix-socket JSONL reuse of the shipped runtime
    protocol, stale-socket cleanup, explicit run or stop entrypoints, and the
    shared client split between spawned stdio children and daemon attaches
- `62-daemon-owned-detached-session-registry.md`
  - daemon-owned detached-session summaries and restart reconciliation, with
    typed `list_detached_sessions` or `inspect_detached_session` protocol
    calls, persisted session-control summaries, and explicit resumable versus
    terminal restart outcomes
- `63-detached-session-watch-and-log-subscriptions.md`
  - the detached-session log and watch lane for Phase 2, including append-only
    daemon event logs, cursor-based replay, authoritative versus best-effort
    truth labels, daemon watch subscriptions, and the shared `probe-client`
    helpers above that event stream
- `64-daemon-operator-cli.md`
  - the first human-facing daemon operator surface for Phase 2, including
    `probe daemon run|stop`, `probe ps|attach|logs|stop`, daemon autostart,
    and the bounded binary-regression coverage for those commands
- `65-detached-watchdog-and-timeout-policy.md`
  - daemon-owned stalled-turn and total-timeout policy for detached sessions,
    including `timed_out` status, per-turn progress metadata, queued follow-up
    cancellation, and the watchdog configuration knobs on daemon startup
