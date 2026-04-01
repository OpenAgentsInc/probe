# Admitted-Mac Comparison Runbook

Issue `#29` adds a separate repo-owned local runbook for the heavy Apple FM
versus Psionic Qwen comparison lane.

## Why It Is Separate

This lane is intentionally not part of the fast PR-safe path.

It depends on:

- admitted Apple hardware
- a reachable Apple FM bridge
- a reachable Psionic Qwen endpoint
- heavier model-dependent comparison receipts

Those assumptions are valid for an explicit local comparison run and invalid
for the normal precommit path.

## Repo Entry Point

Local wrapper:

```bash
./probe-dev accept-compare
```

## Runner Assumptions

The runbook assumes the operator is already on an admitted Apple-silicon Mac
with:

- a reachable Apple FM bridge
- a reachable Psionic Qwen endpoint
- enough local disk for retained comparison artifacts

## Inputs

The local wrapper can optionally override:

- Qwen profile, base URL, and model
- Apple FM profile, base URL, and model

That keeps the default lane simple while still allowing an operator to point
the run at a non-default local bridge or Qwen endpoint when necessary.

## Artifacts

The local run writes the retained comparison bundle from the compare root,
including:

- the comparison report
- the Qwen acceptance report
- the Apple FM acceptance report
- any transcript and report artifacts written under the comparison run root
- the console log for the compare command when the operator captures one

## Failure Posture

The compare command can fail if the comparison lane reports comparable
failures.

That failure is informative for the heavy comparison lane, not a merge-blocking
precommit verdict.

## Operator Reading Guide

When a compare run finishes:

1. read the uploaded comparison report first
2. inspect the embedded per-backend case status
3. use the backend-specific acceptance reports and transcript paths when a case
   diverges
4. treat unsupported posture as explicit comparison scope, not as a generic
   failure rewrite

This keeps the heavy comparison lane repeatable and inspectable without
pretending it belongs in a default GitHub CI contract.
