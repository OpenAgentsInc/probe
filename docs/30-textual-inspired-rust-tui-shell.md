# Bootstrap A Textual-Inspired Rust TUI Shell For Probe

GitHub issue:
`https://github.com/OpenAgentsInc/probe/issues/30`

## Summary

Bootstrap a Textual-inspired Rust TUI shell for Probe so we can render a
retained UI surface, prove the basic screen/event/focus model, and have a
visible terminal UI target to iterate on.

## Why

Probe currently has an interactive CLI loop, but it does not yet have a real
retained terminal UI surface.

The recent TUI audits point to a clear split:

- keep Codex-like runtime and terminal-transcript discipline below the UI seam
- borrow Textual-style UI architecture at the UI seam
- use Toad-like product-shell patterns only after the basic framework seam
  exists

Before approvals, diffs, session pickers, embedded terminals, or a richer
composer, Probe needs a very small honest first TUI lane that proves:

- terminal lifecycle and clean teardown
- a basic app / screen / widget decomposition in Rust
- local UI event routing and key handling
- visible screen updates from state changes
- room for a future screen stack instead of one giant monolithic widget

## Scope

Add the first minimal Probe TUI lane in Rust.

Concrete work:

- add a small TUI crate or equivalently isolated module boundary instead of
  growing the runtime core directly
- define the first Textual-inspired UI primitives in Rust, at least enough to
  separate:
  - app shell
  - screen
  - one or more bounded view/widget regions
  - UI-local events or messages
- add a narrow CLI entrypoint such as `probe tui hello` or equivalent so
  operators can launch the demo intentionally without pretending the full
  interactive CLI is already TUI-backed
- wire terminal lifecycle in a way that leaves the terminal clean on exit and
  failure
- render one hello-world style screen that shows obvious visible structure, at
  minimum:
  - a header/title row
  - a main content region
  - a footer/help row
- make a few keys visibly change state on screen, for example:
  - toggle body text
  - switch between two simple views or tabs
  - open and dismiss a basic modal/help screen
- keep the implementation shaped for future growth into richer screens rather
  than one giant render function
- document how to run the demo locally
- add at least minimal test coverage for the TUI state/update path, and if
  practical add a narrow snapshot or process-level regression test for the
  hello screen

## Non-Goals

Do not build the full Probe chat UI in this issue.

Do not add transcript streaming, tool execution, approvals, diffs, or embedded
terminals yet.

Do not rewrite Probe around Python or Textual itself.

Do not force the final long-term inline-versus-scrollback transcript decision
in this issue.

## Architectural Direction

This issue should follow the recent workspace audits, translated into Rust
rather than Python:

- Codex stays the primary reference for runtime boundary and shell-native
  transcript strategy
- Textual is the primary reference for UI decomposition, screen/widget
  ownership, partial update thinking, selection semantics, and testing posture
- Toad is a product-shell reference, not the runtime boundary

The first implementation can stay simple, but it should avoid painting Probe
into a corner where every future TUI feature must be added to one oversized
`App` object.

## Suggested Shape

A good first target would be something roughly like:

- `probe-tui` crate or similarly isolated module
- `AppShell` or equivalent top-level controller for terminal lifecycle and
  dispatch
- `Screen` trait/type for one active screen at a time
- at least one hello/demo screen plus one secondary modal/help screen or
  alternate view
- a tiny UI event enum for key-driven state changes

The point is not to clone Textual APIs.

The point is to establish the same kind of seam in Rust.

## Done When

- a dedicated Probe TUI demo command launches successfully from the repo
- the demo renders a stable structured screen with visible header/body/footer
  regions
- at least a couple of key presses visibly update on-screen state
- there is at least one additional focused surface or view transition beyond a
  single static screen
- quitting restores the terminal cleanly
- the code shape makes future screens/widgets easier to add than the usual
  one-file monolith
- repo docs explain how to run the TUI demo and what architectural boundary it
  is intended to establish

## References

- Codex TUI rendering audit:
  `https://github.com/AtlantisPleb/workspace/blob/main/docs/probe/10-openai-codex-tui-terminal-ui-rendering-audit.md`
- Toad TUI architecture audit:
  `https://github.com/AtlantisPleb/workspace/blob/main/docs/probe/11-batrachian-toad-tui-architecture-and-lessons.md`
- Textual framework audit:
  `https://github.com/AtlantisPleb/workspace/blob/main/docs/probe/12-textual-framework-terminal-ui-architecture-and-lessons.md`
- existing interactive CLI/resume doc:
  `https://github.com/OpenAgentsInc/probe/blob/main/docs/08-interactive-cli-and-resume.md`
- CLI regression and snapshots doc:
  `https://github.com/OpenAgentsInc/probe/blob/main/docs/21-cli-regression-and-snapshots.md`
