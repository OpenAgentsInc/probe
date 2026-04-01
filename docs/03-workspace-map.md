# Workspace Map

## Current Crates

- `probe-protocol`
  - shared protocol-level types and constants
- `probe-client`
  - shared first-party client layer for spawning `probe-server`, running the
    handshake, and adapting typed server responses back into Probe runtime
    value types
- `probe-core`
  - controller runtime entrypoint and cross-crate coordination surface
- `probe-server`
  - local-first multi-client runtime server boundary with stdio protocol
    handling and workspace lifecycle planning
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
  - terminal entrypoint for Probe, now routed through `probe-client` for the
    primary session loop
- `probe-tui`
  - Textual-inspired Rust terminal UI shell for retained screens, widget-like
    regions, focused modal surfaces, and a background worker now talking to the
    shared client layer instead of the runtime directly

## Early Rule

Keep these crates small and explicit.

Do not let `probe-core` become a catch-all crate for every subsystem.

As the repo grows, new crates should only be introduced when they represent a
real subsystem boundary.
