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
  --hosted-owner-id probe-hosted-control-plane
```

Optional flags:

- `--hosted-display-name <name>`
- `--hosted-attach-target <target>`

If `--hosted-attach-target` is omitted, Probe advertises the bound listener as
`tcp://<resolved-bind-addr>`.

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

## Why This Matters

Hosted Forge consumers need more than "a TCP socket exists."

They need explicit answers for:

- who owns this session right now
- how it should be reattached later
- whether it is local foreground, local detached, or remotely hosted

That ownership truth now lives in the Probe-owned session model instead of in
consumer-side assumptions.

## Current Limits

This issue does not claim:

- hosted worker baseline manifests
- snapshot or restore provenance
- remote execution-host metadata
- auth, tenancy, or cloud policy
- browser or web transport support

Those stay follow-on work above this first hosted control-plane seam.
