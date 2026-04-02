## Goal

Probe now has a typed session-mount contract for Forge follow-on work.

The runtime can accept explicit knowledge-pack and eval-pack refs at session
startup, persist those refs as session metadata, and expose the same mounted
refs later through session snapshots and detached-session summaries.

## Contract

`StartSessionRequest` now accepts `mounted_refs: Vec<SessionMountRef>`.

Each `SessionMountRef` carries:

- a stable `mount_id`
- a typed `kind`
  - `knowledge_pack`
  - `eval_pack`
- a machine-readable `resource_ref`
- an optional human `label`
- typed provenance
  - `publisher`
  - `source_ref`
  - optional `version`
  - optional `content_digest`

That keeps mount identity, type, and source explicit on the Probe-owned runtime
seam instead of burying them in prompt prose.

## Persistence And Projection

Probe persists mounted refs in `SessionMetadata`.

That means the refs now flow through:

- `start_session`
- `resume_session`
- `inspect_session`
- detached-session registry summaries
- child sessions spawned from a mounted parent session

Detached consumers can inspect the mounted pack set without scraping transcript
text or app-owned side state.

## Refusal Posture

Probe currently accepts only two mount kinds:

- `knowledge_pack`
- `eval_pack`

If a client submits any unsupported mount kind, `probe-server` now refuses the
request explicitly with protocol error code `unsupported_session_mount_kind`
before the session is persisted.

Probe also rejects malformed mounts when:

- `mount_id` is empty
- the same `mount_id` appears more than once
- `resource_ref` is empty
- provenance fields such as `publisher` or `source_ref` are empty

## Boundary

This does not make Probe the pack catalog owner.

Probe still does not own:

- pack authoring UX
- pack selection policy
- pack search
- hosted catalog storage

It only owns the runtime truth that a session started with a particular set of
typed mounts and that those mounts remained attached to that session.
