# Harness Profiles

## Purpose

Probe now has Probe-owned harness profiles for the coding tool lane.

The first profile exists to make the active controller prompt explicit,
versioned, and replayable instead of hiding it inside an ad hoc `--system`
string.

## First Profile

The first shipped harness profile is:

- name: `coding_bootstrap_default`
- version: `v1`
- compatible tool set: `coding_bootstrap`

It injects:

- session cwd
- operating-system and shell-family hints
- coding-lane operating rules
- tool-usage guidance
- output-budget guidance
- edit-and-verify guidance
- activity-bound session hygiene guidance

## CLI Surface

`probe exec` and `probe chat` now accept:

- `--harness-profile coding_bootstrap_default`

Example:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --harness-profile coding_bootstrap_default \
  "Read README.md and summarize the repository."
```

If `--tool-set coding_bootstrap` is selected and no explicit harness profile is
provided, Probe selects `coding_bootstrap_default@v1` automatically.

## Relation To `--system`

`--system` still exists, but it is no longer the only way to shape controller
behavior.

The current rule is:

- if a harness profile is active, Probe renders the profile-owned system prompt
- if `--system` is also provided, Probe appends it as an operator addendum
- if no harness profile is active, Probe falls back to the raw `--system`
  string when present

This keeps the baseline behavior Probe-owned while still allowing local
operator notes.

## Persistence

Session metadata now records the active harness profile name and version.

That matters because later replay, eval, and GEPA work need to compare runs
against a stable baseline instead of guessing which prompt variant was active.

`probe chat --resume` treats the stored harness profile as authoritative and
does not accept `--harness-profile` overrides on resume.

## Why This Matters

Harness profiles are the stable comparison surface for later optimization.

Probe should not treat "prompt craft" as hidden operator behavior if it wants
to compare:

- harness variants
- routing policies
- verification policies
- oracle escalation behavior

The first profile keeps that future work attached to durable runtime truth.
