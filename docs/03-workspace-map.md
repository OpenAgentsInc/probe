# Workspace Map

## Current Crates

- `probe-protocol`
  - shared protocol-level types and constants
- `probe-core`
  - controller runtime entrypoint and cross-crate coordination surface
- `probe-provider-openai`
  - backend client crate for OpenAI-compatible local backends
- `probe-cli`
  - terminal entrypoint for Probe

## Early Rule

Keep these crates small and explicit.

Do not let `probe-core` become a catch-all crate for every subsystem.

As the repo grows, new crates should only be introduced when they represent a
real subsystem boundary.
