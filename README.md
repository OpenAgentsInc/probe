![Protoss Probe](assets/images/protossprobe.jpg)

# Probe

Probe is a coding-agent runtime for software work. It is being built as a
Rust-first controller with a CLI, durable local session state, a typed runtime
protocol, bounded tool execution, and a clean seam to local or remote model
backends.

The repo currently contains five crates: `probe-protocol`, `probe-core`,
`probe-provider-openai`, `probe-decisions`, and `probe-cli`. The default local
backend lane is `psionic-qwen35-2b-q8-registry`, targeting
`http://127.0.0.1:8080/v1` with the model id
`qwen3.5-2b-q8_0-registry.gguf`.

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
`ToolRoute` and `PatchReadiness`.

Probe can either attach to an already-running local backend or launch
`psionic-openai-server` as a supervised child process. It also records basic
controller-side observability on model-generated turns, including wallclock,
usage when available, derived completion throughput, and a conservative
cache-signal heuristic. More detailed design and implementation notes live
under `docs/`.

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
observability fields.

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
emit observability lines on stderr for model-generated turns and print the
active harness profile when one is selected. Tool-backed runs also emit a
policy summary for auto-allowed, approved, refused, and paused tool calls.
