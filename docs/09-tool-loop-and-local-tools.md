# Tool Loop And Local Tools

## Purpose

Probe now has a bounded local tool runtime.

The current implementation is intentionally small and deterministic:

- local built-in tool sets
- one bounded controller loop
- append-only transcript storage for tool calls and tool results
- replay into later model turns through the normal `chat.completions` surface

## Built-In Tool Set

Probe currently ships two built-in tool sets.

### `coding_bootstrap`

This is the canonical local coding lane.

It declares:

- `read_file`
  - reads a bounded line range from a relative text file in the session cwd
- `list_files`
  - lists relative directory contents with bounded depth and entry count
- `code_search`
  - wraps `rg` for bounded code search inside the session cwd
- `shell`
  - runs a bounded shell command in the session cwd
- `apply_patch`
  - applies deterministic text replacement to a relative file
- `consult_oracle`
  - when configured, consults a bounded auxiliary model for planning,
    checking, or research guidance
- `analyze_repository`
  - when configured, runs a bounded long-context repo-analysis pass over
    explicit evidence files for architecture, synthesis, or change-impact work

This is the first honest coding-tool bundle for Probe.

### `weather`

The retained demo tool set is:

- `weather`
  - declares one tool: `lookup_weather`
  - accepts one argument: `city`
  - returns retained demo weather for:
    - `Paris`
    - `Tokyo`

This is enough to prove:

- required single-tool turns
- replay of tool results into a later model turn
- same-turn two-call batches when parallel tool calls are enabled

## CLI Surface

Both `probe exec` and `probe chat` now accept:

- `--tool-set weather`
- `--tool-set coding_bootstrap`
- `--tool-choice <none|auto|required|named:<tool>>`
- `--parallel-tool-calls`
- `--approve-write-tools`
- `--approve-network-shell`
- `--approve-destructive-shell`
- `--pause-for-approval`
- `--oracle-profile <name>`
- `--oracle-max-calls <n>`
- `--long-context-profile <name>`
- `--long-context-max-calls <n>`
- `--long-context-max-evidence-files <n>`
- `--long-context-max-lines-per-file <n>`

Example:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --tool-choice auto \
  "Read README.md and summarize the repository."
```

Parallel batch example:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --tool-choice auto \
  --parallel-tool-calls \
  "Search for runtime crates and then read the root README."
```

## Runtime Flow

1. Rebuild prior conversation messages from session metadata and transcript.
2. Send the current user turn to the backend.
3. If the backend returns assistant tool calls:
   - persist a `tool_call` turn
   - classify each requested tool call against the local approval policy
   - for `analyze_repository`, also enforce the long-context escalation gate
     based on task shape, explicit evidence paths, and session context pressure
   - auto-allow, approve, refuse, or pause each tool call
   - execute only the calls allowed by policy
   - persist a `tool_result` turn
   - replay executed or refused results into the next model request
   - stop early if the active policy pauses on approval
4. Stop when the backend returns a normal assistant message.
5. Refuse infinite loops with a fixed model-round-trip bound.

## Transcript Contract

Transcript items now carry tool-specific fields:

- `name`
- `tool_call_id`
- structured `arguments` for `tool_call` items
- structured `tool_execution` records for `tool_result` items

The `tool_execution` record carries fields such as:

- `risk_class`
- `policy_decision`
- `approval_state`
- `command`
- `exit_code`
- `timed_out`
- `truncated`
- `bytes_returned`
- `files_touched`
- `reason`

For long-context repo-analysis calls, the tool result body also carries:

- the selected analysis profile and model
- the bounded evidence file list
- per-file truncation and line metadata
- the analysis text returned by the auxiliary lane

That lets Probe reconstruct:

- assistant tool-call messages
- tool result messages
- later resume context after a tool-backed session already happened

## Current Boundary

The current built-in tool lanes intentionally do not try to solve everything:

- no plugin marketplace
- no arbitrary external executors
- no streaming tool deltas
- no unbounded multi-agent planner
- no default long-context fallback for ordinary coding turns

It is the smallest honest tool-backed controller loop that:

- uses declared tools
- preserves stable replay
- stays compatible with the retained local backend lane
- makes the local policy boundary explicit instead of hiding it in ad hoc shell conventions
