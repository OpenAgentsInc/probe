![Protoss Probe](assets/images/protossprobe.jpg)

# Probe

Probe is a coding agent runtime for software work.

It is intended to run in three modes:

- interactive terminal sessions
- non-interactive execution for scripted or batch tasks
- long-lived local or remote server mode for supervised sessions

## Goals

- one runtime that can serve multiple client surfaces
- structured session, turn, and item models
- durable transcripts and indexed runtime state
- safe tool execution with explicit permissions and approvals
- strong workspace and project awareness
- a small, stable machine-readable protocol between the runtime and its clients

## Initial Scope

The first versions of Probe should focus on:

- a Rust-first core runtime
- a small local server surface
- a CLI on top of the same runtime
- append-only session records plus lightweight indexed metadata
- a typed tool runtime
- clear policy boundaries around approvals, execution, and sandboxing

## Repo Layout

The repository now includes a Rust workspace with:

- `probe-protocol`
- `probe-core`
- `probe-provider-openai`
- `probe-cli`

Planning docs live under `docs/`.

## Non-Goals For The First Milestone

- a large plugin marketplace
- broad cloud control-plane features
- multiple overlapping runtime implementations
- product-shell concerns that belong in client applications

## Status

This repository is in bootstrap stage.

The near-term objective is to establish the Rust workspace, define the runtime
boundary, and build the first end-to-end session loop on a clean protocol and
persistence foundation.
