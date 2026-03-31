# Interactive CLI And Resume

## Purpose

`probe chat` is the first interactive controller surface in this repo.

It is built on the same `ProbeRuntime` used by `probe exec`.

That matters because the interactive lane should not become a second runtime
implementation with different session semantics.

## Command Shape

Start a new session:

```bash
cargo run -p probe-cli -- chat
```

Resume an existing session:

```bash
cargo run -p probe-cli -- chat --resume <session_id>
```

Optional flags for a new session:

- `--profile <name>`
- `--cwd <path>`
- `--title <text>`
- `--system <text>`
- `--probe-home <path>`

Resume deliberately does not accept new title or system prompt overrides.

The stored session settings remain authoritative.

## Runtime Flow

For a fresh session:

1. Wait for the first non-empty prompt line.
2. Create a session on the first real turn.
3. Persist backend metadata and any system prompt in session metadata.
4. Append each user and assistant turn to the transcript.

For a resumed session:

1. Load session metadata by id.
2. Recover the stored backend profile name from session metadata.
3. Replay the stored transcript into a new `chat.completions` request.
4. Append the new turn to the same stable session id.

## Transcript Replay Rule

The current replay logic rebuilds conversation context from:

- persisted system prompt
- prior `user_message` items
- prior `assistant_message` items

It intentionally ignores:

- `note` items
- tool items

That is correct for the pre-tool interactive lane.

The tool-aware replay contract will expand in the later tool issue.

## Terminal Behavior

The current chat surface uses clearly separated turn output instead of
incremental streaming.

Supported local commands:

- `/help`
- `/quit`
- `/exit`

## Why This Shape

This is the smallest honest interactive lane that preserves:

- stable session identity
- durable local state
- shared runtime objects across exec and chat
- transcript-backed resume instead of fake stateless chat
