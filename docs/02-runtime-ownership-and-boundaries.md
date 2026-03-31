# Runtime Ownership And Boundaries

## Purpose

This document defines what Probe owns and what it consumes from adjacent
systems.

The goal is to keep the controller cleanly separated from the serving and model
substrate.

## Probe Owns

Probe owns the controller runtime:

- session lifecycle
- turn loop
- transcript truth
- local metadata index
- CLI and later server surfaces
- tool execution policy at the controller layer
- user-facing runtime behavior

## Probe Does Not Own

Probe does not own model serving substrate work that already belongs elsewhere.

That includes:

- GGUF family parsing
- tokenizer families
- prompt-template family implementation
- CUDA or CPU decode kernels
- local model-loading runtime
- OpenAI-compatible serving transport

## Psionic Boundary

For the first backend-consumption lane, Probe should consume `psionic` through
the existing OpenAI-compatible HTTP seam.

That means:

- Psionic owns serving a model
- Probe owns driving the conversation and tool loop against that model

The first canonical backend path is:

- `psionic-openai-server`
- local Qwen3.5 GGUF
- `chat.completions`

## Early Design Rule

Do not reimplement substrate work in Probe before the controller exists.

The first Probe value is:

- being a Rust controller with durable local truth

not:

- being a second model server
