# Probe Exec

## Purpose

`probe exec` is the first honest end-to-end controller lane in this repo.

It is intentionally narrow:

- one-shot plain-text request
- one backend profile
- one persisted local session
- one terminal response path

## Command Shape

```bash
cargo run -p probe-cli -- exec "Summarize the current directory"
```

Optional flags:

- `--profile <name>`
  - backend profile name
- `--cwd <path>`
  - working directory recorded in the session metadata
- `--title <text>`
  - explicit session title instead of a prompt-derived one
- `--system <text>`
  - optional system prompt for the request
- `--probe-home <path>`
  - override the default Probe home used for local transcript persistence

## Runtime Flow

1. Resolve the named backend profile.
2. Resolve Probe home from `--probe-home`, `PROBE_HOME`, or `~/.probe`.
3. Create a local session with backend metadata attached.
4. Send a plain `chat.completions` request through the typed provider client.
5. Persist the resulting turn as append-only transcript data.
6. Print assistant text to stdout and session details to stderr.

## Session Persistence

`probe exec` persists:

- session metadata
- backend target metadata
- append-only transcript events

The first successful plain-text turn stores:

- one `user_message` item
- one `assistant_message` item

If the backend request fails after session creation, Probe persists a `note`
item alongside the user message so the local transcript still records the
failure context.

## Shared Runtime Objects

The CLI now uses `ProbeRuntime` from `probe-core`.

That runtime object is the intended shared seam for:

- `probe exec`
- the future interactive session loop
- session resume
- later tool runtime work
