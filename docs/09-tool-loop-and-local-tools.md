# Tool Loop And Local Tools

## Purpose

Probe now has its first bounded tool runtime.

The current lane is intentionally small and deterministic:

- one local built-in tool set
- one bounded controller loop
- append-only transcript storage for tool calls and tool results
- replay into later model turns through the normal `chat.completions` surface

## Built-In Tool Set

The first shipped tool set is:

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
- `--tool-choice <none|auto|required|named:lookup_weather>`
- `--parallel-tool-calls`

Example:

```bash
cargo run -p probe-cli -- exec \
  --tool-set weather \
  --tool-choice required \
  "What is the weather in Paris?"
```

Parallel batch example:

```bash
cargo run -p probe-cli -- exec \
  --tool-set weather \
  --tool-choice required \
  --parallel-tool-calls \
  "Check Paris and Tokyo."
```

## Runtime Flow

1. Rebuild prior conversation messages from session metadata and transcript.
2. Send the current user turn to the backend.
3. If the backend returns assistant tool calls:
   - persist a `tool_call` turn
   - execute the declared local tools in order
   - persist a `tool_result` turn
   - replay those results into the next model request
4. Stop when the backend returns a normal assistant message.
5. Refuse infinite loops with a fixed model-round-trip bound.

## Transcript Contract

Transcript items now carry tool-specific fields:

- `name`
- `tool_call_id`
- structured `arguments` for `tool_call` items

That lets Probe reconstruct:

- assistant tool-call messages
- tool result messages
- later resume context after a tool-backed session already happened

## Current Boundary

The first tool lane intentionally does not try to solve everything:

- no plugin marketplace
- no arbitrary external executors
- no streaming tool deltas
- no unbounded multi-agent planner

It is the smallest honest tool-backed controller loop that:

- uses declared tools
- preserves stable replay
- stays compatible with the retained local backend lane
