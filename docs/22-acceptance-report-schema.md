# Acceptance Report Schema

Probe's acceptance report is now a first-class eval artifact rather than a
thin pass/fail receipt.

The current schema version is `v3`.

## Run-Level Structure

The report now carries four top-level groups:

- `run`
  - schema version
  - generated run id
  - Probe version
  - best-effort git SHA and dirty-state provenance
- `backend`
  - backend profile name
  - base URL
  - model id
- `harness`
  - tool set
  - harness profile name and version
  - repeat-run count
- `counts`
  - total and passed/failed counts for both cases and attempts

The report also retains:

- `started_at_ms`
- `finished_at_ms`
- `duration_ms`
- `overall_pass`
- `results`

## Case-Level Structure

Each case now records:

- case name and stable case index
- pass/fail plus passed-attempt and failed-attempt counts
- median elapsed time when available
- latest session id and transcript path
- latest assistant text
- latest executed tool count
- latest tool-name set
- latest policy-count summary
- latest observability summary
- latest backend-receipt summary when the final turn carried one
- first relevant failure category when a case truly fails
- retained attempt list

## Attempt-Level Structure

Each attempt now records:

- attempt index
- pass/fail
- failure category when applicable
- session id and transcript path when available
- assistant text when available
- executed tool-call count
- tool-name list
- policy-count summary
- observability summary when available
- backend-receipt summary when available
- error text when the attempt emitted one

## Observability And Receipts

The observability summary now preserves both:

- best-effort scalar token counts
- optional per-field `value` plus `truth`, where truth is `exact` or
  `estimated`

That matters for Apple FM because the bridge may only be able to offer
estimated usage.

The backend-receipt summary is intentionally narrower than the raw stored
receipt. It keeps comparison-safe facts such as:

- typed failure family, code, retryability, refusal text, or tool name
- availability readiness plus reason code when the backend surfaced one
- transcript receipt format plus payload byte count

The report does not inline full backend transcript payloads. Probe's own
stored transcript remains the authoritative runtime record.

## Failure Categories

The report now separates the main non-success classes into:

- `backend_failure`
- `tool_execution_failure`
- `policy_refusal`
- `policy_paused`
- `verification_failure`
- `configuration_failure`
- `unknown_failure`

This is intentionally narrow. The point is to make acceptance drift legible
enough for later replay export, candidate comparison, and harness tuning
without turning the report into a bespoke dashboard format.
