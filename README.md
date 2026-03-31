![Protoss Probe](assets/images/protossprobe.jpg)

# Probe

Probe is a coding-agent runtime for software work. It is being built as a
Rust-first controller with a CLI, durable local session state, a typed runtime
protocol, bounded tool execution, and a clean seam to local or remote model
backends.

The repo currently contains seven crates: `probe-protocol`, `probe-core`,
`probe-provider-openai`, `probe-provider-apple-fm`, `probe-decisions`,
`probe-optimizer`, and `probe-cli`. The default local backend lane is
`psionic-qwen35-2b-q8-registry`, targeting `http://127.0.0.1:8080/v1` with the
model id `qwen3.5-2b-q8_0-registry.gguf`. Probe now also ships the first real
Apple FM backend lane through `psionic-apple-fm-bridge`, targeting
`http://127.0.0.1:8081` with the model id `apple-foundation-model`.

## Status Snapshot

Probe is now past the bootstrap/demo stage and into the first real local
coding-runtime stage.

Shipped now:

- `probe exec` and `probe chat` on the shared runtime/session model
- append-only transcript persistence plus local resume
- the `coding_bootstrap` tool lane with approvals, harness profiles, and acceptance cases
- replay and decision-dataset export
- narrow offline-evaluable decision modules plus optimizer receipts
- bounded oracle consultation and bounded long-context repo-analysis escalation
- local backend attach and supervised launch flows
- Apple FM plain-text `exec`/`chat` turns, Apple-FM-backed `consult_oracle`,
  and session-backed Apple FM coding turns through the Probe approval layer

Current posture:

- local-first
- single-controller
- transcript- and policy-driven
- optimized for honest coding turns before any larger recursive or multi-agent work

Still intentionally not the goal:

- plugin marketplace sprawl
- hidden recursive runtimes
- default long-context escalation for ordinary coding tasks
- opaque optimizer magic in the hot path

## Current State

Probe already works as a first local controller stack. You can run one-shot
turns with `probe exec`, interactive multi-turn sessions with `probe chat`,
resume prior sessions by id, persist append-only transcripts under
`PROBE_HOME` or `~/.probe`, run a retained local acceptance harness, and use a
bounded built-in tool runtime including same-turn parallel tool calls.

The canonical local coding lane is now `coding_bootstrap`, which ships the
first real built-in coding tools:

- `read_file`
- `list_files`
- `code_search`
- `shell`
- `apply_patch`
- `consult_oracle`
  - only when an auxiliary oracle profile is configured
- `analyze_repository`
  - only when an auxiliary long-context profile is configured

The retained `weather` tool set remains available as a tiny regression fixture.

Probe also now has a Probe-owned harness profile for that coding lane:

- `coding_bootstrap_default@v1`

This keeps the default controller prompt explicit and versioned instead of
relying only on raw `--system` strings.

The coding lane now also has explicit local approval classes. By default:

- `read_file`, `list_files`, `code_search`, and read-only `shell` commands are auto-allowed
- `apply_patch`, write-class shell commands, networked shell commands, and destructive shell commands are refused unless explicitly approved
- `--pause-for-approval` switches denied risky tool calls from refusal into a persisted pending-approval pause

Probe persists structured tool-result records for coding sessions, including
risk class, policy decision, approval state, command metadata, truncation,
bytes returned, and touched paths when known.

Above that runtime lane, Probe now has a narrow Rust-native decision-module
crate for offline module evaluation. The first module families are
`ToolRoute`, `PatchReadiness`, and `LongContextEscalation`.

Probe also now supports a bounded auxiliary oracle lane through a typed
`consult_oracle` tool. Oracle calls stay inside the main controller loop as
tool invocations rather than becoming a second controller. The auxiliary oracle
can now target either the current Psionic Qwen lane or the Apple FM bridge lane.

Probe also now supports a bounded long-context repo-analysis lane through a
typed `analyze_repository` tool. This path is opt-in, budgeted, and only
allowed for explicit repo-analysis tasks once the session has enough evidence
or obvious context pressure.

Probe can either attach to an already-running local backend or launch
`psionic-openai-server` as a supervised child process. It also records basic
controller-side observability on model-generated turns, including wallclock,
best-effort usage, exact-versus-estimated usage truth when the backend can say,
derived completion throughput, and a conservative cache-signal heuristic.
Probe now also has a typed backend-receipt slot for adjunct evidence such as
Apple FM transcript exports or typed refusal and availability facts. More
detailed design and implementation notes live under `docs/`.

The Apple FM lane now overlaps honestly with the Qwen coding lane, but it is
still not identical:

- `probe exec` and `probe chat` support plain-text Apple FM turns
- `consult_oracle` can target an Apple FM profile
- tool-backed coding turns on Apple FM now run through a Probe-owned callback
  server and the same local approval policy
- Apple FM resume rebuilds session continuity from Probe transcript state each
  turn instead of depending on stored backend session ids
- managed launch remains OpenAI-compatible only, and Apple FM does not claim
  explicit OpenAI-style parallel tool-call control

For local validation, Probe now has a canonical runner script at the repo
root: `./probe-dev fmt`, `./probe-dev check`, `./probe-dev test`, and
`./probe-dev accept`. The test command prefers `cargo nextest run
--no-fail-fast` when `cargo nextest` is installed and falls back to
`cargo test --workspace` otherwise.

Probe now also has binary-level CLI regression tests and narrow normalized
snapshots for `exec` stderr, selected transcript receipts, and the acceptance
report shape.

The acceptance report itself now carries run identity, git provenance, backend
and harness metadata, aggregate counts, typed failure categories, transcript
references, and final-turn observability truth summaries so it can serve as a
real local eval receipt.

The repo-local operator split is now explicit: use `./probe-dev pr-fast` for
the fast merge-safe lane, `./probe-dev cli-regressions` for binary output and
snapshot work, and `./probe-dev accept-live` plus the eval wrappers for the
heavier local acceptance and research lanes.

## Commands

Build and validation:

```bash
cargo test -p probe-provider-openai -p probe-core -p probe-cli
cargo check
```

One-shot execution:

```bash
cargo run -p probe-cli -- exec "Explain what this repository does."
```

Interactive session:

```bash
cargo run -p probe-cli -- chat
```

Resume a prior session:

```bash
cargo run -p probe-cli -- chat --resume <session-id>
```

Tool-enabled execution:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --harness-profile coding_bootstrap_default \
  --tool-choice auto \
  "Read README.md and summarize what this repository does."
```

Parallel tool-call batch:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --approve-write-tools \
  --tool-choice auto \
  "Update hello.txt by replacing world with probe."
```

Pause instead of refusing a risky tool call:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --pause-for-approval \
  --tool-choice auto \
  "Patch hello.txt to say probe instead of world."
```

Parallel tool-call batch:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --tool-choice auto \
  --parallel-tool-calls \
  "Search for the runtime crate names and then read the README."
```

Acceptance harness:

```bash
cargo run -p probe-cli -- accept
```

The acceptance runner now targets retained `coding_bootstrap` cases instead of
only the old weather demo. Its JSON report includes repeat-run receipts,
median wallclock, per-attempt tool-policy counts, and final-turn
observability fields, including exact-versus-estimated usage detail when the
backend reports it.

Dataset export:

```bash
cargo run -p probe-cli -- export \
  --dataset decision \
  --output ~/.probe/reports/probe_decision.jsonl
```

By default, export targets coding-lane sessions. Add `--all-sessions` to widen
scope or `--session <id>` to export one specific session.

Offline module evaluation:

```bash
cargo run -p probe-cli -- module-eval \
  --dataset ~/.probe/reports/probe_decision.jsonl
```

Offline module optimization receipts:

```bash
cargo run -p probe-cli -- optimize-modules \
  --dataset ~/.probe/reports/probe_decision.jsonl \
  --output ~/.probe/reports/probe_module_optimization.json
```

Harness candidate comparison:

```bash
cargo run -p probe-cli -- optimize-harness \
  --baseline-report ~/.probe/reports/probe_acceptance_baseline.json \
  --candidate-report ~/.probe/reports/probe_acceptance_candidate.json \
  --output ~/.probe/reports/probe_harness_optimization.json
```

Oracle-enabled coding session:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --oracle-profile psionic-qwen35-2b-q8-oracle \
  --oracle-max-calls 1 \
  "Ask the oracle for a checking recommendation before editing."
```

Apple FM plain-text session:

```bash
cargo run -p probe-cli -- exec \
  --profile psionic-apple-fm-bridge \
  "Summarize the Probe runtime boundary."
```

Apple FM tool-backed coding session:

```bash
cargo run -p probe-cli -- exec \
  --profile psionic-apple-fm-bridge \
  --tool-set coding_bootstrap \
  --tool-choice required \
  "Read hello.txt and tell me what it says."
```

Apple FM oracle inside the Qwen coding lane:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --oracle-profile psionic-apple-fm-oracle \
  --oracle-max-calls 1 \
  "Consult the Apple FM oracle before deciding what file to inspect."
```

Long-context repo-analysis session:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --long-context-profile psionic-qwen35-2b-q8-long-context \
  --long-context-max-calls 1 \
  --long-context-max-evidence-files 6 \
  --long-context-max-lines-per-file 160 \
  "If this turns into a repo-analysis task, use analyze_repository with explicit evidence paths."
```

Explicit attach mode:

```bash
cargo run -p probe-cli -- exec \
  --server-mode attach \
  "Reply with exactly ATTACHED_OK."
```

Launch mode:

```bash
cargo run -p probe-cli -- exec \
  --server-mode launch \
  --server-binary /path/to/psionic-openai-server \
  --server-model-path /path/to/model.gguf \
  --server-model-id qwen3.5-2b-q8_0-registry.gguf \
  "Reply with exactly LAUNCHED_OK."
```

By default, Probe uses the `psionic-qwen35-2b-q8-registry` profile and
`attach` server mode. Session transcripts, server config, and acceptance
reports live under the Probe home directory. `probe exec` and `probe chat`
emit observability lines on stderr for model-generated turns, print exact or
estimated usage truth when available, surface backend-receipt summaries when a
turn carries one, and print the active harness profile when one is selected.
Tool-backed runs also emit a policy summary for auto-allowed, approved,
refused, and paused tool calls.
