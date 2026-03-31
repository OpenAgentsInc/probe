![Protoss Probe](assets/images/protossprobe.jpg)

# Probe

Probe is a coding agent runtime for software work.

It is intended to run in three modes:

- interactive terminal sessions
- non-interactive execution for scripted or batch tasks
- long-lived local or remote server mode for supervised sessions

## Goals

- one runtime that can serve multiple client surfaces
- structured session, turn, and item models
- durable transcripts and indexed runtime state
- safe tool execution with explicit permissions and approvals
- strong workspace and project awareness
- a small, stable machine-readable protocol between the runtime and its clients

## Initial Scope

The first versions of Probe should focus on:

- a Rust-first core runtime
- a small local server surface
- a CLI on top of the same runtime
- append-only session records plus lightweight indexed metadata
- a typed tool runtime
- clear policy boundaries around approvals, execution, and sandboxing

## Repo Layout

The repository now includes a Rust workspace with:

- `probe-protocol`
- `probe-core`
- `probe-provider-openai`
- `probe-cli`

Planning docs live under `docs/`.

The first canonical backend profile is a local Psionic-served Qwen lane:

- profile: `psionic-qwen35-2b-q8-registry`
- base URL: `http://127.0.0.1:8080/v1`
- model id: `qwen3.5-2b-q8_0-registry.gguf`

The first end-to-end CLI lane is `probe exec`, which:

- sends a plain-text request to the configured backend profile
- prints the assistant answer to the terminal
- persists the resulting session transcript under `PROBE_HOME` or `~/.probe`

The first interactive lane is `probe chat`, which:

- starts a local REPL on top of the same runtime objects used by `probe exec`
- supports resuming a prior session by id
- rebuilds conversation context from the persisted transcript before each turn

The first tool-enabled lane now exists on both `probe exec` and `probe chat`:

- enable the built-in retained demo tool set with `--tool-set weather`
- control tool policy with `--tool-choice`
- enable same-turn batches with `--parallel-tool-calls`

Probe also has a retained acceptance runner:

- `probe accept`
- writes a JSON report for the current local backend target
- is designed to hit a local Psionic-served Qwen lane when one is available

Probe now supports both local server attachment and supervised launch:

- attach to an already-running local server with the default `--server-mode attach`
- launch `psionic-openai-server` as a child process with `--server-mode launch`
- persist the effective local server config under the Probe home

Probe now records first-pass controller observability on model-generated turns:

- per-turn request wallclock and model-output timing
- prompt, completion, and total token counts when the backend returns usage
- derived completion throughput and a conservative cache-signal heuristic
- operator-readable observability lines on both `probe exec` and `probe chat`

## Status

Probe now has a working first local controller stack.

What is possible now:

- run one-shot plain-text turns against a local OpenAI-compatible backend
- run interactive multi-turn chat sessions and resume them by session id
- persist append-only transcripts plus indexed session metadata under
  `PROBE_HOME` or `~/.probe`
- drive a bounded local tool loop with a retained demo tool set
- exercise same-turn parallel tool-call batches
- run a local acceptance harness against the configured backend lane
- attach to an already-running local backend or launch
  `psionic-openai-server` as a supervised child process
- inspect basic controller-side performance signals for model-generated turns

## Relevant Commands

Build and test:

```bash
cargo test -p probe-provider-openai -p probe-core -p probe-cli
cargo check
```

Run a one-shot turn against an already-running local backend:

```bash
cargo run -p probe-cli -- exec "Explain what this repository does."
```

Start an interactive session:

```bash
cargo run -p probe-cli -- chat
```

Resume an existing interactive session:

```bash
cargo run -p probe-cli -- chat --resume <session-id>
```

Run with the retained demo tool set:

```bash
cargo run -p probe-cli -- exec \
  --tool-set weather \
  --tool-choice required \
  "Use the weather tool for Paris and answer with the result."
```

Enable same-turn parallel tool calls:

```bash
cargo run -p probe-cli -- exec \
  --tool-set weather \
  --tool-choice required \
  --parallel-tool-calls \
  "Use the weather tool for Paris and Tokyo in the same turn."
```

Run the local acceptance harness:

```bash
cargo run -p probe-cli -- accept
```

Attach to an already-running local backend explicitly:

```bash
cargo run -p probe-cli -- exec \
  --server-mode attach \
  "Reply with exactly ATTACHED_OK."
```

Launch `psionic-openai-server` as a supervised child process:

```bash
cargo run -p probe-cli -- exec \
  --server-mode launch \
  --server-binary /path/to/psionic-openai-server \
  --server-model-path /path/to/model.gguf \
  --server-model-id qwen3.5-2b-q8_0-registry.gguf \
  "Reply with exactly LAUNCHED_OK."
```

Operator notes:

- the default backend profile is `psionic-qwen35-2b-q8-registry`
- the default server mode is `attach`
- session transcripts and reports live under the Probe home directory
- `probe exec` and `probe chat` emit observability lines on stderr for
  model-generated turns

## Non-Goals For The First Milestone

- a large plugin marketplace
- broad cloud control-plane features
- multiple overlapping runtime implementations
- product-shell concerns that belong in client applications
