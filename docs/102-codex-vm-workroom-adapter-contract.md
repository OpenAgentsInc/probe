# Codex VM Workroom Adapter Contract

Status: canonical
Last updated: 2026-06-01
Owner: OpenAgents
Canonical successor: none
Related docs:
- `54-openai-codex-subscription-auth.md`
- `55-openai-codex-subscription-backend.md`
- `95-managed-environment-contract.md`
- `../../docs/cloud/codex-vm-workroom.md`
- `../../docs/cloud/issues.md`
- `../../vortex/docs/workroom-runner-service.md`

## Purpose

Probe owns the durable coding-agent runtime semantics for Codex VM workrooms.
Cloud may keep a temporary thin runner for the first GCP smoke, and Vortex may
own the product UX, account connection, Program Runs, review, approval, and
acceptance. The handoff between them should still use one Probe-compatible
runtime contract so the temporary runner can be replaced without changing the
Autopilot/Vortex surface.

The schema implemented in `probe-protocol` is:

```text
probe.codex_workroom.v1
```

This contract is intentionally narrower than the full managed runtime API. It
is the normalized event and artifact shape for a Codex-backed workroom runner.
Vortex decides acceptance; Probe and Cloud only report execution evidence.

The richer per-turn, per-tool, per-shell-command stream for SHC training and
continuation is `probe.codex_managed_event.v1`, documented in
`110-codex-managed-runtime-event-contract.md`. Use that stream when Vortex
needs replay, failure learning, signature package evidence, or continuation
checkpoints.

## Session Model

`CodexWorkroomSessionSpec` carries the declared execution boundary:

| Field | Meaning |
| --- | --- |
| `workroomId` | Product workroom identity supplied by Vortex/Autopilot. |
| `sessionId` | Probe/runner session identity for replay and closeout. |
| `threadId` | Optional Codex thread id when Codex exposes one. |
| `repoRef` | Repository/ref checked out for the workroom. |
| `cwd` | Runner working directory. |
| `sandboxMode` | `workspace_write` for the first MVP. |
| `approvalPolicy` | Initial MVP uses `never`; future modes can pause for approval. |
| `timeoutMs` | Hard execution bound. |
| `authProfileRef` | Provider-account auth grant ref, never raw tokens. |
| `artifactPolicy` | `metadata_only`, `redacted_logs`, or a stricter local policy. |
| `callbackTarget` | Vortex/Cloud callback target for normalized events. |
| `mode` | `codex_exec`, `codex_mcp_server`, or `codex_sdk_thread`. |

Raw ChatGPT/Codex tokens, `~/.codex/auth.json`, API keys, GCP credentials,
local `.secrets` paths, and shell environments are not valid trace material.
The protocol redaction helpers remove these shapes before event persistence.

## Initial Adapter

The first supported runtime is:

```text
workroom assignment
  -> CodexExecAdapter
  -> codex exec --json --sandbox workspace-write
  -> probe.codex_workroom.v1 events
  -> artifactRefs / receiptRefs / closeout summary
```

The adapter normalizes Codex CLI JSONL, stderr progress, command summaries,
file-change summaries, MCP-looking responses, and SDK-looking events into the
same event vocabulary:

```text
queued
started
log
redacted
artifact
receipt
completed
failed
timeout
cancelled
```

Non-JSON stderr/stdout becomes a redacted `log` event. `file_change`,
`patch_apply`, and `artifact_ref` become `artifact` events. `turn_completed`
and closeout events become `completed` or `receipt` events with `receiptRefs`.

## Artifact And Receipt Shape

Artifact refs include:

| Field | Meaning |
| --- | --- |
| `path` | Runner-local or storage resource ref after redaction. |
| `digest` | Stable digest when available. |
| `mimeType` | MIME or coarse artifact type. |
| `visibility` | `private`, `workroom`, or `public_projection`. |
| `retention` | `ephemeral`, `retained`, or `snapshot_on_finish`. |
| `producer` | `codex_exec`, `cloud_runner`, or a future adapter id. |
| `closeoutRef` | Receipt or closeout resource that explains the artifact. |

Receipt refs include:

| Field | Meaning |
| --- | --- |
| `receiptType` | Example: `workroom.closeout`. |
| `resourceRef` | Durable receipt reference. |
| `digest` | Optional stable digest. |

Vortex should render these as evidence references for review, not as proof of
accepted work by themselves.

## Failure Semantics

Failures normalize into:

| Failure kind | Use |
| --- | --- |
| `nonzero_exit` | Codex exited unsuccessfully after starting. |
| `timeout` | Runner hit the declared timeout. |
| `cancelled` | Operator or policy cancelled the workroom. |
| `auth_failure` | Codex auth/login/account state failed. |
| `setup_failure` | VM or Codex setup failed before execution. |
| `artifact_capture_failure` | Execution may have run, but evidence capture failed. |
| `stream_disconnect` | Event stream broke before terminal closeout. |
| `unknown` | Last resort; should be rare. |

Terminal event kinds are `completed`, `failed`, `timeout`, and `cancelled`.
Non-terminal events are only progress until a terminal closeout arrives.

## Cloud Compatibility

Cloud's temporary `oa-workroomd` Codex runner should emit the same event
semantics. The Probe module accepts Cloud-style runner records with `kind` or
`type`, `message`, `artifactRefs`, and `receiptRefs`, then maps them to
`probe.codex_workroom.v1`.

That means the first Cloud path can be:

```text
Vortex /api/workrooms/start
  -> Cloud control API
  -> oa-workroomd codex run
  -> Cloud runner events
  -> Probe-compatible Codex workroom events
  -> Vortex workroom timeline
```

## Follow-On Modes

`CodexMcpAdapter` should run `codex mcp-server` with explicit `cwd`, sandbox,
approval policy, and optional `threadId` so Vortex/Cloud can submit follow-up
turns without shelling arbitrary commands.

`CodexSdkThreadAdapter` should persist a Codex thread id, stream SDK events,
support richer closeout artifacts, and resume a workroom after runner restart.
It should still emit the same event kinds and artifactRefs/receiptRefs.

## Guardrails

- Do not persist raw `auth.json` content.
- Do not put wallet authority in Codex workrooms.
- Do not treat a successful process exit as accepted work.
- Do not let Cloud or Probe become Autopilot product authority.
- Do not expose local machine paths in public projections.
- Do not accept arbitrary shell text from Vortex as the execution contract.
