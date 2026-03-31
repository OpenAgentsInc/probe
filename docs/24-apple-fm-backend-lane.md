# Apple FM Backend Lane

## Purpose

Probe now has a second real backend family:

- `open_ai_chat_completions`
- `apple_fm_bridge`

This is not a profile rename.

The Apple FM lane is wired through its own provider crate and its own backend
kind so Probe can consume the real `psionic-apple-fm` substrate honestly.

## What Landed

Probe now supports Apple FM for:

- plain-text `probe exec`
- plain-text `probe chat`
- bounded `consult_oracle`

Built-in profiles:

- `psionic-apple-fm-bridge`
- `psionic-apple-fm-oracle`

The default local bridge expectation is:

- `http://127.0.0.1:8081`
- model id `apple-foundation-model`

## Server Attach Boundary

Probe's server preparation path is now backend-aware.

For the current two lanes:

- Qwen/OpenAI-compatible attach waits for `GET /v1/models`
- Apple FM attach checks `GET /health`

If the Apple FM bridge reports the model unavailable, Probe now fails with the
typed unavailability reason instead of flattening the condition into generic
transport noise.

Managed launch remains OpenAI-compatible only. Apple FM is attach-only for now.

## Provider Boundary

Probe now has a shared plain-text provider dispatch in `probe-core`:

- OpenAI-compatible requests still route through `probe-provider-openai`
- Apple FM requests route through `probe-provider-apple-fm`

That shared dispatch is used by:

- the plain-text session/runtime path
- `consult_oracle`
- the bounded repo-analysis helper

This is the smallest honest seam that lets Probe consume Apple FM for plain
text, bounded oracle use, and backend attach without forcing the OpenAI tool
wire shape onto the backend.

## Relationship To The Coding Lane

The later Apple FM coding-tool lane now lives separately in
`docs/25-apple-fm-tool-lane.md`.

That follow-up adds:

- session-backed tool registration
- callback-based tool execution through Probe's approval layer
- Probe-owned replay of Apple transcript state for resume

This document remains the retained record for the first plain-text, oracle, and
attach boundary.
