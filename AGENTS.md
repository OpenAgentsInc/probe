# AGENTS

This file defines the shared agent contract for the `probe` repo.

## Purpose

`probe` owns the coding-agent runtime itself.

That includes:

- runtime session lifecycle
- protocol and event model
- tool execution
- permission and approval policy
- persistence and recovery
- CLI and server surfaces

This repo should not quietly absorb unrelated product-shell concerns.

## Working Rules

- Keep the runtime boundary explicit.
- Prefer a small set of focused crates over one oversized core crate.
- Treat the machine-readable protocol as a first-class product surface.
- Keep session, turn, item, and task models explicit in code and docs.
- Separate execution policy from sandbox and executor mechanics.
- Prefer append-only runtime artifacts plus indexed metadata over opaque blobs.

## Early Priorities

- establish a Rust workspace
- define the protocol types
- build the first local server mode
- build the CLI on the same client/runtime contract
- add durable session storage
- add the first typed tool and approval flows

## Start Here

- `README.md`

Add more canonical docs here as the repo grows.
