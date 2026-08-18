# Probe

Probe is the first-party OpenAgents coding agent runtime: one sans-I/O
Rust agent core, compiled native and to WebAssembly, wrapped by
Effect/TypeScript host packages, speaking Agent Client Protocol (ACP) v1
as a server, with pluggable model transports — Sarah's inference-grant
provider proxy, local OpenAI-compatible endpoints (Psionic), and direct
API keys. It is launched on paired machines by `sarah-computer-controller`
and ships as the pinned `@openagentsinc/probe` npm package.

The architecture, salvage ledger, conformance contract, and execution
plan live in `docs/2026-08-18-zerobase-rust-core-audit-and-spec.md`.
Work is tracked in issue #213.

Two prior Probes are preserved in Git history as source material, never
as a base: the 2026 Rust workspace (archived in `92134ae`) and the
TypeScript/Effect runtime (archived in the commit that introduced this
README). Recover salvage with `git show <archive-commit>^:<path>`.
