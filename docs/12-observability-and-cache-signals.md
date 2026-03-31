# Probe Observability And Cache Signals

Probe now records a small per-turn observability payload on every
model-generated turn.

That includes:

- plain assistant-answer turns
- model turns that emit tool calls
- later assistant turns after tool results are replayed

It does not attach model observability to pure tool-result transcript turns,
because those are local controller execution, not backend generation.

## What Gets Recorded

The persisted field is `SessionTurn.observability`.

The current schema records:

- `wallclock_ms`
  - full request wallclock as measured by the controller
- `model_output_ms`
  - the current model-output timing window
  - in the initial non-streaming lane this is equal to `wallclock_ms`
- `prompt_tokens`
- `completion_tokens`
- `total_tokens`
  - only present when the backend returns usage data
- `prompt_tokens_detail`
- `completion_tokens_detail`
- `total_tokens_detail`
  - optional `value` plus `truth`
  - `truth` is currently `exact` or `estimated`
  - Probe keeps the plain token fields as the backend's best-effort scalar so
    older operator paths can still read a single number quickly
- `completion_tokens_per_second_x1000`
  - completion throughput scaled by `1000`
  - `15342` means `15.342` completion tokens per second
- `cache_signal`
  - `cold_start`
  - `likely_warm`
  - `no_clear_signal`
  - `unknown`

## How The Cache Signal Works

Probe does not claim to own backend cache internals.

The first cache signal is a controller-side heuristic meant to make repeated
turn behavior legible enough to debug regressions.

Current rules:

- first observed model turn in a session is `cold_start`
- if the current turn has no prompt-token data, the signal is `unknown`
- if the current turn has prompt-token data, Probe compares it to the last
  prior prompt-bearing model turn in the same session
- if the current turn carries at least as many prompt tokens and finishes at
  least `20%` faster, the signal becomes `likely_warm`
- otherwise the signal is `no_clear_signal`

This is intentionally conservative.

Probe is trying to answer a narrow operator question:

"Does this repeated turn look materially faster than the last comparable turn
in the same session?"

## Operator Surface

Both `probe exec` and `probe chat` now print one stderr line per model turn:

```text
observability wallclock_ms=118 model_output_ms=118 prompt_tokens=24(exact) completion_tokens=12(estimated) total_tokens=36(estimated) completion_tps=101.694 cache_signal=likely_warm
```

That line is meant to be readable in local terminal sessions and easy to grep
from captured logs.

If a turn also carries backend-specific adjunct evidence, Probe emits a second
summary line such as:

```text
backend_receipt transcript_format=foundation_models.transcript.v1 transcript_payload_bytes=842
```

That second line is intentionally narrow. It confirms what extra receipt was
attached without pretending the receipt replaces the authoritative Probe
transcript.

## Reading The First Signals

Use the fields in this order:

1. `wallclock_ms`
   - start here for end-to-end turn latency
2. `prompt_tokens`
   - tells you whether the later turn carried at least as much context as the
     baseline turn
   - if `prompt_tokens_detail.truth=estimated`, treat the cache signal as
     heuristic above a heuristic rather than backend truth
3. `completion_tps`
   - tells you whether decoded output speed changed materially
4. `cache_signal`
   - a quick summary of whether the repeated turn looks warm enough to be
     interesting

Interpretation guidance:

- `cold_start` on a first turn is expected
- `likely_warm` is the useful positive signal for repeated-turn reuse
- `no_clear_signal` means the run did not show an obvious warm-path win
- `unknown` usually means the backend omitted usage data, so Probe could not
  make a prompt-aware comparison

## Current Limits

- the first lane is blocking and non-streaming, so Probe cannot yet separate
  time-to-first-token from decode duration
- throughput depends on backend usage counters being present
- exact-versus-estimated truth is only as good as the provider contract feeding
  Probe
- the cache signal is local-session and heuristic, not a backend-truth cache
  metric

That is acceptable for the first Psionic-backed CLI milestone.

The point is to make repeated-turn performance visible immediately without
inventing a heavy telemetry system before the core controller exists.
