# Probe TUI Real Runtime Session Worker

## Summary

Issue `#42` replaces the Probe TUI temporary reply worker with the real
`probe-core` runtime.

`cargo probe` now keeps one live Probe session in the TUI:

- the first submit calls `ProbeRuntime::exec_plain_text`
- later submits call `continue_plain_text_session`
- the default chat lane uses `coding_bootstrap` plus the existing conservative
  approval policy
- committed transcript rows now come from the persisted Probe session

This is the first honest runtime-backed TUI loop. The shell still lacks live
tool lifecycle streaming and real approval resume, but prompt submit no longer
goes through a fake assistant worker.

## What Changed

### Worker request model

`probe-tui` no longer queues `TranscriptDemoReply`.

It now queues `ProbeRuntimeTurn`, which carries:

- `probe_home`
- `cwd`
- backend profile
- system prompt and harness profile
- tool-loop config

That request is enough for the worker thread to run a real Probe turn without
reconstructing shell state inside the UI layer.

The same worker bridge now also accepts a typed `SelectGithubIssue` request for
the prompt-as-priority path. That request discovers GitHub-backed sibling repos
from the current workspace, fetches open issues with `gh issue list`, and runs
the typed issue-selection signature from `probe-decisions`.

That path is now gated before dispatch. Probe only queues GitHub issue lookup
for work-shaped prompts, not for casual chat such as `who are you`.

Issue lookup deliberately runs on a detached helper thread so the TUI can queue
issue selection before the runtime turn without stalling the actual coding
roundtrip.

### Session-backed runtime loop

The worker now keeps retained runtime session state:

- persisted `session_id`
- `probe_home`
- `cwd`
- backend profile name
- how many transcript events have already been rendered into the TUI

If the config changes, the worker drops the prior session and starts fresh.
Otherwise it reuses the existing session and resumes it on the next submit.

### Transcript hydration from Probe truth

After each successful runtime turn, the worker reads the session transcript
from the Probe session store and emits only the newly appended events back into
the TUI.

The TUI now renders committed rows derived from Probe transcript items:

- assistant message
- tool call
- tool result
- runtime note

User messages are not duplicated from the session store because the TUI already
commits the user turn immediately when the composer submits.

GitHub issue selection results are also rendered as first-class status rows in
the transcript. If the signature selects an issue, the footer header picks up
`repo#number title`. If no issue matches, the transcript records that outcome
without inventing stale metadata.

### Honest error handling

If the runtime fails after creating or mutating a session, the worker now tries
to recover the session id, session metadata, and any newly persisted transcript
before surfacing the error.

That keeps the TUI honest about partial progress instead of dropping all
runtime truth on the floor when a turn fails.

## Current Boundaries

This issue does not add live tool lifecycle streaming.

Today the TUI still shows:

- an immediate generic active-turn cell while the runtime works
- committed transcript rows only after the runtime turn finishes or fails

Issue `#43` is the next step for parity-feel: typed runtime events for tool
call request, execution start, execution completion, refusal, pause, and final
assistant commit.

## Tests

Coverage now proves:

- composer submission queues a real Probe runtime turn instead of the old temporary
  worker label
- a TUI submit can drive a tool-backed runtime turn against a fake OpenAI
  backend and commit the resulting transcript rows
- later submits reuse the same runtime session id
- snapshots reflect the new runtime sidebar and transcript fixtures

Validation:

```bash
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
cargo check --workspace
```
