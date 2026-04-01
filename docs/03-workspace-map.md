# Workspace Map

## Current Crates

- `probe-protocol`
  - shared protocol-level types and constants
- `probe-core`
  - controller runtime entrypoint and cross-crate coordination surface
- `probe-provider-openai`
  - backend client crate for OpenAI-compatible local backends
- `probe-provider-apple-fm`
  - Apple Foundation Models provider boundary and bridge integration seam
- `probe-decisions`
  - offline-evaluable decision-module boundary above the runtime
- `probe-optimizer`
  - Psionic bridge artifacts, promotion ledgers, and bounded optimize-anything
    families above the runtime
- `probe-cli`
  - terminal entrypoint for Probe
- `probe-tui`
  - Textual-inspired Rust terminal UI shell for retained screens, widget-like
    regions, and focused modal surfaces

## Early Rule

Keep these crates small and explicit.

Do not let `probe-core` become a catch-all crate for every subsystem.

As the repo grows, new crates should only be introduced when they represent a
real subsystem boundary.
