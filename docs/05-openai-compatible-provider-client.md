# OpenAI-Compatible Provider Client

## Purpose

This document defines the first backend client seam for Probe.

The goal is to keep the first backend lane:

- narrow
- explicit
- easy to debug

## Initial Contract

The first provider client targets:

- local OpenAI-compatible endpoints
- `chat.completions`
- plain text turns first

The first config surface includes:

- `base_url`
- `model`
- `api_key`
- `timeout`
- `stream`

## Current Boundary

The first implementation supports non-streaming `chat.completions` requests.

If streaming is enabled in config today, the client returns an explicit
unsupported-streaming error instead of pretending that the feature works.

That keeps the first controller/backend seam honest while still carrying the
streaming knob in the config model.

## Why This Shape

Probe needs a backend seam before it needs a backend abstraction tower.

The first useful thing is:

- one typed client crate that can talk to a local OpenAI-compatible server

That is enough for:

- `probe exec`
- the first interactive session loop
- the first Psionic consumption lane
