# Hosted TCP Transport And Runtime Ownership

Issue `#91` adds the first hosted Probe control-plane lane above the shipped
local daemon seam.

## What Landed

Probe now supports the same runtime protocol over three transport shapes:

- stdio JSONL
- local daemon Unix-socket JSONL
- hosted TCP JSONL

The hosted path is intentionally narrow.

It is a Rust-to-Rust control-plane transport for remote Probe consumers. It is
not yet a browser-facing HTTP API, a multi-tenant auth layer, or a worker
scheduler.

## Server Entry Point

`probe-server` now accepts:

```bash
probe-server \
  --probe-home ~/.probe \
  --listen-tcp 127.0.0.1:7777 \
  --hosted-owner-id probe-hosted-control-plane \
  --watchdog-stall-ms 180000
```

Optional flags:

- `--hosted-display-name <name>`
- `--hosted-attach-target <target>`
- `--watchdog-poll-ms <ms>`
- `--watchdog-stall-ms <ms>`
- `--watchdog-timeout-ms <ms>`

If `--hosted-attach-target` is omitted, Probe advertises the bound listener as
`tcp://<resolved-bind-addr>`.

Hosted TCP now accepts the same watchdog knobs as the local daemon path, which
matters for Codex-backed remote turns that can go quiet for longer than the
default detached stall window without actually being dead.

## Client Transport

`probe-client` now supports:

- `ProbeClientTransportConfig::HostedTcp { address }`

That transport:

- opens a plain TCP JSONL connection
- runs the same initialize handshake
- uses the same typed request, response, and event shapes
- does not auto-start a remote server
- does not shut the hosted server down on drop

That keeps the remote lane honest. Hosted attach is explicit; it is not hidden
behind local fallback behavior.

## Runtime Ownership Metadata

Probe now persists typed runtime-owner state on sessions.

`SessionMetadata.runtime_owner` records:

- owner kind
  - `foreground_child`
  - `local_daemon`
  - `hosted_control_plane`
- owner id
- attach transport
  - `stdio_jsonl`
  - `unix_socket_jsonl`
  - `tcp_jsonl`
- optional display name
- optional attach target

Detached summaries now mirror that same owner metadata so remote consumers can
inspect ownership without scraping daemon files or guessing from transport.

Hosted sessions now also persist typed workspace provenance alongside that
owner metadata:

- boot mode
- prepared baseline id and readiness or stale state
- snapshot or restore refs
- execution-host metadata
- explicit fresh-start fallback notes when a requested prepared baseline is not
  actually available

## Why This Matters

Hosted Forge consumers need more than "a TCP socket exists."

They need explicit answers for:

- who owns this session right now
- how it should be reattached later
- whether it is local foreground, local detached, or remotely hosted

That ownership truth now lives in the Probe-owned session model instead of in
consumer-side assumptions.

## Current Limits

This issue still does not claim:

- hosted worker scheduling
- auth, tenancy, or cloud policy
- browser or web transport support

Those stay follow-on work above the first hosted control-plane and provenance
seams.
