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
