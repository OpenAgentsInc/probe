# Backend Receipts And Usage Truth

Issue `#27` widened Probe's turn-level observability and receipt model so Apple
FM can be compared honestly against the Psionic Qwen lane without flattening
important backend-specific truth.

## Problem

The earlier Probe observability lane assumed token counts were small exact
integers and that the common transcript model was enough to explain backend
behavior.

That was acceptable for the initial Psionic Qwen path, but it lost important
Apple FM facts:

- usage may be estimated rather than exact
- token counts should not silently truncate to `u32`
- typed refusal or unavailability categories should survive persistence
- Apple FM session lanes can return backend transcript exports that are useful
  adjunct evidence even though Probe's own transcript remains authoritative

## Data Model

Probe now records two related but distinct turn-level structures.

### `TurnObservability`

The common observability payload now keeps:

- `prompt_tokens`, `completion_tokens`, and `total_tokens` as `u64`
- per-field `*_detail` values with:
  - `value`
  - `truth`
  - `truth = exact | estimated`

The scalar fields stay because operator surfaces and older tooling still want a
single easy number.

The detail fields exist so later reports and comparison tools do not have to
guess whether the number was exact.

### `SessionTurn.backend_receipt`

Each stored turn can also carry an optional backend receipt with three narrow
families:

- `failure`
  - typed backend failure or refusal facts
- `availability`
  - typed availability or readiness facts
- `transcript`
  - backend-native transcript export metadata plus payload

This slot is adjunct evidence, not the source of truth for the session.

Probe's own append-only transcript is still the canonical runtime record.

## Current Producers

### OpenAI-compatible Psionic Qwen lane

- stores exact usage detail for prompt, completion, and total tokens
- does not currently attach backend receipts

### Apple FM plain-text lane

- stores best-effort usage counts with exact-versus-estimated detail when the
  bridge provides it
- persists typed backend failure receipts on provider errors
- maps `assets_unavailable` into both a failure receipt and an availability
  receipt

### Apple FM session tool lane

- stores best-effort usage counts with detail truth
- persists typed refusal or failure receipts on backend-originated errors
- attaches a backend transcript receipt on successful assistant turns when the
  bridge returns an Apple transcript export

## Operator Surface

`probe exec` and `probe chat` now render usage like:

```text
prompt_tokens=42(exact)
prompt_tokens=9(estimated)
```

When a turn carries adjunct backend evidence, the CLI prints a compact
`backend_receipt ...` line that summarizes only stable facts such as:

- failure family and code
- retryability
- availability readiness and reason code
- transcript format and payload byte count

The CLI intentionally does not dump large backend payloads inline.

## Acceptance Reports

The acceptance report moved to schema `v3`.

It now preserves:

- observability detail truth for token fields
- summarized backend receipts on attempts and case-level latest results

The report still avoids embedding full backend transcript payloads. It records
only the transcript format and payload size so future comparison runs can tell
whether adjunct evidence existed without bloating the report.

## Boundary

The common Probe contract should hold facts that every backend can reasonably
share:

- turn latency
- throughput
- best-effort token counts
- exact-versus-estimated usage truth
- local policy outcomes
- transcript-visible user, assistant, tool-call, and tool-result items

Backend receipts are for facts that would otherwise be lost but should not be
forced into the common transcript grammar:

- backend-native transcript exports
- backend-specific availability facts
- typed refusal or failure categories that are useful to preserve verbatim

That keeps Probe transcript state authoritative while still letting future
Apple FM versus Qwen comparisons remain honest about what each backend really
reported.
