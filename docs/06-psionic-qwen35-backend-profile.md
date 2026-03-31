# Psionic Qwen3.5 Backend Profile

## Purpose

Probe now carries one explicit built-in backend profile for the first supported
local lane.

This avoids hiding the first real backend target in undocumented environment
variables or ad hoc CLI flags.

## Canonical Profile

- profile name: `psionic-qwen35-2b-q8-registry`
- transport: OpenAI-compatible `chat.completions`
- base URL: `http://127.0.0.1:8080/v1`
- model id: `qwen3.5-2b-q8_0-registry.gguf`
- API key env var: `PROBE_OPENAI_API_KEY`
- timeout: `45s`
- server mode: `attach_to_existing`
- prefix cache mode: `backend_default`

## Why These Defaults

`attach_to_existing` is the correct first mode because the earliest Probe
controller milestone should be able to talk to a running backend without also
owning backend process supervision.

`backend_default` is the correct first cache mode because local prefix reuse is
important, but the cache mechanics still belong to the backend.

The controller should record and expose cache intent without pretending it owns
the backend cache implementation.

## Code Surfaces

- `crates/probe-protocol/src/backend.rs`
  - machine-readable backend profile and policy types
- `crates/probe-core/src/backend_profiles.rs`
  - built-in named local backend profiles
- `crates/probe-provider-openai/src/lib.rs`
  - conversion from backend profile into the typed OpenAI-compatible client

## Immediate Use

The CLI can now construct its default provider config from the canonical
profile rather than from an unstructured hard-coded localhost placeholder.
