# Managed Tool Permission Policy

Issue `#140` makes Probe's managed tool contract explicit enough for Laravel,
the TUI, and other first-party clients to reason about every tool call without
parsing runtime internals.

## Registry Manifest

`ToolRegistry::managed_tool_manifest(&approval)` returns a serializable manifest
for the active tool set. Each entry includes:

- tool name and description
- provider input schema
- Probe result schema
- declared or possible risk classes
- timeout policy
- execution owner
- redaction behavior

For `coding_bootstrap`, Probe owns execution through `probe_runtime`. `shell` is
dynamic-risk: each command is classified as `shell_read_only`, `write`,
`network`, or `destructive` before policy is evaluated.

## Permission Model

Probe now normalizes managed policy into three decisions:

- `allow`: execute the tool call
- `ask`: persist a pending approval and pause the turn
- `deny`: return a normal refused tool result

`ToolApprovalConfig` still supports the existing coarse controls:

- `allow_write_tools`
- `allow_network_shell`
- `allow_destructive_shell`
- `denied_action`

Those become the default allow/ask/deny decision per risk class. The same config
also accepts scoped `ToolPermissionOverride` entries, each optionally matching a
tool name, a risk class, or both. More specific overrides win; later overrides
break ties.

The stdio/hosted runtime API carries these overrides through
`ToolApprovalRecipe.overrides`, so Laravel or another admin API can configure
the same policy programmatically.

## Approval Persistence

When policy returns `ask`, Probe:

1. appends the original tool call turn
2. appends a refused-like tool result with `policy_decision = paused`
3. persists a `PendingToolApproval`
4. returns an approval-paused runtime response

Approval resolution remains durable and idempotent in the session store. The
local approval record keeps raw arguments so a restarted Probe can resume the
exact call after approval.

API and UI surfaces must use the redacted form. `probe-server` redacts pending
approval `arguments` before returning `TurnPaused`, `ListPendingApprovals`,
detached-session events, or session snapshots. The raw local store is not the
remote display contract.

## Redacted Summaries

Probe exposes:

- `tool_input_summary(tool_name, arguments)`
- `tool_output_summary(tool_name, output)`

These summaries redact secret-looking keys, token-like values, provider keys,
authorization tokens, and local host paths. They also bound large text previews.

Website-safe runtime events include:

- `argumentsHash`
- `argumentsSummary`
- `outputHash`
- `outputSummary`
- policy decision, approval state, risk class, and touched-file count

That is the Laravel persistence contract for managed-agent UI/history. Full
raw tool input/output remains local Probe execution truth unless a future
artifact policy explicitly exports it.

## Coverage

The focused policy suite covers:

- default refusal for writes
- approval pauses
- scoped allow/ask/deny overrides
- git/network shell classification
- manifest schema/timeout ownership
- redacted summaries
- shell timeout recording
