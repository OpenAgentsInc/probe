# Admitted-Mac Comparison Workflow

Issue `#29` adds a separate repo-owned workflow for the heavy Apple FM versus
Psionic Qwen comparison lane.

## Why It Is Separate

This lane is intentionally not part of the fast PR-safe path.

It depends on:

- admitted Apple hardware
- a reachable Apple FM bridge
- a reachable Psionic Qwen endpoint
- heavier model-dependent comparison receipts

Those assumptions are valid for a dedicated comparison workflow and invalid for
the normal merge-safe CI contract.

## Repo Entry Points

Local wrapper:

```bash
./probe-dev accept-compare
```

GitHub Actions workflow:

- `.github/workflows/apple-fm-qwen-compare.yml`

That workflow runs:

```bash
./probe-dev accept-compare \
  --probe-home "$RUNNER_TEMP/probe-acceptance-compare/probe-home" \
  --report-path "$RUNNER_TEMP/probe-acceptance-compare/probe_acceptance_compare.json"
```

## Trigger Modes

The workflow is intentionally explicit:

- manual `workflow_dispatch`
- scheduled weekly at `09:23 UTC` every Monday

It is not triggered on every PR.

## Runner Assumptions

The workflow requires a self-hosted runner carrying these labels:

- `self-hosted`
- `macOS`
- `apple-silicon`
- `admitted-mac`

Those labels are the contract that the lane is running on the right class of
machine rather than pretending the comparison is portable to generic CI.

## Inputs

Manual dispatch can optionally override:

- Qwen profile, base URL, and model
- Apple FM profile, base URL, and model

That keeps the default lane simple while still allowing an operator to point
the workflow at a non-default local bridge or Qwen endpoint when necessary.

## Artifacts

The workflow always uploads the retained comparison bundle from the compare
root, including:

- the comparison report
- the Qwen acceptance report
- the Apple FM acceptance report
- any transcript and report artifacts written under the comparison run root
- the console log for the compare command

Artifact upload happens even when the comparison command returns a non-zero
exit code.

## Failure Posture

The workflow can fail if the comparison lane reports comparable failures.

That failure is informative for the heavy comparison lane, not a merge-blocking
PR-safe verdict.

The separation is visible in three ways:

- the workflow is manual or scheduled only
- the runner labels are admitted-Mac specific
- the workflow summary explicitly says the lane is not the merge-blocking PR
  path

## Operator Reading Guide

When a workflow run finishes:

1. read the uploaded comparison report first
2. inspect the embedded per-backend case status
3. use the backend-specific acceptance reports and transcript paths when a case
   diverges
4. treat unsupported posture as explicit comparison scope, not as a generic
   failure rewrite

This keeps the heavy comparison lane repeatable and inspectable without
polluting the fast default CI contract.
