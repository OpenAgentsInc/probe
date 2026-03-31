# Session Turn Item And Transcript Model

## Purpose

This document defines the first durable local truth model for Probe.

The target is a controller-friendly structure that is:

- simple enough for the first CLI milestone
- durable enough for resume and listing
- append-only at the transcript layer

## Core Objects

### Session

A session is the durable unit of controller work.

The first metadata model carries:

- session id
- title
- cwd
- created and updated timestamps
- lifecycle state
- next turn number
- transcript path

### Turn

A turn is the unit of one user-driven interaction.

The first turn model carries:

- turn id
- turn index
- start timestamp
- optional completed timestamp
- all items emitted inside the turn

### Item

An item is the visible durable unit inside a turn.

The initial item kinds are:

- user message
- assistant message
- tool call
- tool result
- note

Tool-backed items may also carry structured metadata beyond raw text, such as:

- tool arguments on `tool_call` items
- tool execution and policy records on `tool_result` items

## Storage Layout

The first filesystem layout is:

```text
<root>/
  index.json
  sessions/
    <session_id>/
      metadata.json
      transcript.jsonl
```

`metadata.json` is the current per-session snapshot.

`transcript.jsonl` is append-only controller truth.

`index.json` is the lightweight listing index used for session listing and
resume lookup without scanning every transcript file.

## Append-Only Rule

The transcript file is append-only.

Probe may refresh metadata and index snapshots, but transcript events should be
written as new lines rather than rewritten in place.

## Why This Model

This keeps the first local truth split explicit:

- transcript events are the durable source trail
- metadata and index files are the queryable controller summary layer

That is enough for:

- `probe exec`
- interactive resume
- later tool-loop history replay
