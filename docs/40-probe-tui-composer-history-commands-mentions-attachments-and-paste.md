# Probe TUI Composer History, Commands, Mentions, Attachments, And Paste

## Summary

Issue #40 extends the Probe TUI composer from a plain text box into a real
draft model.

The draft subsystem now has explicit support for:

- draft history recall
- slash-command detection
- typed mention parsing
- attachment placeholders
- paste-aware multiline input

## What Changed

### Draft model

`BottomPane` no longer owns only a text buffer and cursor. The draft state now
tracks:

- text
- cursor position
- attachment placeholders
- submission history
- whether the current draft came from a multiline or large paste

Slash commands and mentions are derived explicitly from the current draft text
instead of being treated as incidental raw characters.

### History recall

The composer now supports shell-style history recall with `Up` and `Down`.

History restores prior submitted drafts rather than only prior text lines, so
future richer draft state can continue to hang off the same seam.

### Slash commands and typed mentions

The composer now exposes explicit extension points for future command and
binding behavior:

- leading `/command` is recognized as a slash-command draft
- typed mentions such as `@skill:rust`, `@app:github`, and
  `@runtime:session` are parsed into typed mention records

The current UI surfaces those bindings in the composer metadata line and in the
committed user turn after submit.

Exact zero-argument shell commands now stay local to the TUI:

- `/help`
- `/backend`
- `/approvals`
- `/reasoning`
- `/clear`

That means the hot path keeps the simpler shell behavior from current `main`
instead of importing PR `#107`'s heavier command palette. Slash-prefixed text
with extra arguments or natural-language content still submits as a normal
Probe runtime turn.

### Attachments

`Ctrl+O` now adds an attachment placeholder to the draft.

This is intentionally small and local-first, but it gives Probe a real
non-text draft element owned by the input subsystem rather than by transcript
rendering or shell status code.

### Paste-aware handling

Probe now handles terminal paste explicitly through a `ComposerPaste` event
path instead of implicitly treating pasted content as only many single-key
inserts.

Large or multiline pastes mark the draft as paste-derived, which is visible in
composer metadata and committed transcript output.

## Tests

Coverage now includes:

- shell-style history recall
- attachment placeholder submission
- slash-command and typed-mention extraction
- exact local slash-command handling for help, backend, reasoning, and clear
- explicit paste-aware submission behavior

Validation commands:

```bash
cargo test -p probe-tui -- --nocapture
cargo test -p probe-cli --test cli_regressions -- --nocapture
```
