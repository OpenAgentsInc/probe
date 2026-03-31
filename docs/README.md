# Probe Docs

This folder holds technical planning docs for the Probe runtime.

## Table Of Contents

- `01-psionic-qwen-hermes-deep-dive-and-probe-cli-roadmap.md`
  - deep dive on the prior Psionic Hermes/Qwen work and the concrete roadmap
    for consuming that backend from the first Rust Probe CLI
- `02-runtime-ownership-and-boundaries.md`
  - ownership line for what Probe should own itself and what it should consume
    from the backend substrate
- `03-workspace-map.md`
  - initial crate map for the Probe Rust workspace
- `04-session-turn-item-and-transcript-model.md`
  - the first durable local truth model for sessions, turns, items, and
    append-only transcript storage
- `05-openai-compatible-provider-client.md`
  - the first typed backend client seam for local OpenAI-compatible endpoints
- `06-psionic-qwen35-backend-profile.md`
  - the first explicit built-in backend profile for a local Psionic-served
    Qwen3.5 model
- `07-probe-exec.md`
  - the first non-interactive end-to-end Probe controller lane with local
    transcript persistence
- `08-interactive-cli-and-resume.md`
  - the first interactive session loop and transcript-backed resume flow
- `09-tool-loop-and-local-tools.md`
  - the first bounded local tool runtime, batch execution path, and replay
    contract
