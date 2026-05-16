# OpenAgents Coder Runtime Adapter Compatibility

Issue: [#142](https://github.com/OpenAgentsInc/probe/issues/142)  
Autopilot adapter:
[#199](https://github.com/OpenAgentsInc/autopilot3/issues/199)  
Autopilot tracker:
[#200](https://github.com/OpenAgentsInc/autopilot3/issues/200)

Probe can be used as a hosted OpenAgents Coder runtime behind the same Coder
job/event/approval contract as the Codex app-server, Flue, local companion, and
future runtime adapters. Probe remains the runtime boundary. Autopilot remains
the product boundary that owns workspaces, Coder deployments, Coder jobs,
approvals, evidence, receipts, and user-visible API responses.

## Stable Contract

Hosted Probe should expose these Probe-owned contracts to Autopilot:

- `probe.managed_runtime.v1` for start, resume, replay, cancel, resolve
  approval, heartbeat, child-session, transcript-ref, and artifact-ref control.
- `probe.website_event.v1` for product-safe event streaming into Autopilot's
  `coder.event.v1` rows.
- `probe.managed_environment.v1` for hosted worker advertisement, capability
  matching, environment provenance, and incompatibility reasons.
- `probe.forge_worker.verification_pack_report` when Autopilot or Forge needs
  release-gate evidence for hosted worker identity and sync state.

Autopilot should treat hosted Probe as one runtime adapter behind Coder. It
should not depend on Probe internals, raw session transcripts, local filesystem
paths, model-provider secrets, or unbounded tool output.
Compatibility shorthand: raw Probe transcripts stay runtime-local unless Probe
exports an explicit redacted artifact ref.

## Operation Mapping

| Coder adapter need | Probe contract |
| --- | --- |
| Start a Coder job | `ManagedRuntimeRequest::StartSession` |
| Attach or resume a job | `ManagedRuntimeRequest::ResumeSession` |
| Replay events after a cursor | `ManagedRuntimeRequest::ReplayEvents` |
| Cancel or interrupt work | `ManagedRuntimeRequest::CancelSession` or `InterruptSession` |
| Resolve a pending approval | `ManagedRuntimeRequest::ResolveApproval` |
| Track hosted worker liveness | `ManagedRuntimeRequest::Heartbeat` |
| Spawn bounded child sessions | `ManagedRuntimeRequest::RecordChildSession` |
| Return evidence pointers | `ManagedRuntimeArtifactRef` and website artifact refs |

Every request should carry Autopilot correlation in
`ManagedRuntimeCorrelation`: `requestId`, `workspace`, `managedRunId`,
`managedSessionId`, `managedEnvironmentId`, `workOrderId`, optional schedule
fields, parent/child Probe session ids, and the Coder job id in metadata.

## Website Event Mapping

Autopilot can map `probe.website_event.v1` into `coder.event.v1` with this
lossless product-safe projection:

| Probe website event | Coder event |
| --- | --- |
| `run_started` | `job.started` |
| `text_delta` | `assistant.delta` |
| `tool_call_started` | `tool.started` |
| `tool_call_completed` | `tool.completed` |
| `approval_requested` | `approval.requested` |
| `approval_resolved` | `approval.resolved` |
| `child_session_started` | `child_session.started` |
| `child_session_updated` | `child_session.updated` |
| `artifact_ref` | `artifact.ref` |
| `runtime_progress` | `runtime.progress` |
| `run_completed` | `job.completed` |
| `run_failed` | `job.failed` |
| `run_cancelled` | `job.canceled` |

Payloads must stay bounded and redacted. Store stable hashes, counts, labels,
risk classes, approval states, status summaries, and artifact refs. Do not
store raw tool arguments, raw tool output blobs, terminal logs, model-provider
tokens, browser cookies, refresh tokens, clone credentials, local paths, or raw
Probe transcripts in Autopilot product state.

## Auth State

Probe-owned Codex subscription auth should be exposed to Autopilot only as
product-safe status:

- `available`, `reauth_required`, `rate_limited`, `disabled`, or `unknown`;
- non-secret account label or route id;
- last checked timestamp;
- usage or limit posture when Probe can summarize it safely;
- retry/reauth instructions that do not include raw paths or tokens.

Raw auth files, session cookies, refresh tokens, model-provider keys, and
subscription account payloads stay runtime-local to Probe or its worker secret
store. Autopilot should never persist them in Coder jobs, Coder deployments,
receipts, API responses, or workspace audit events.

## Hosted Worker Identity And Policy

Hosted Probe workers should advertise:

- worker id, pool id, deployment revision, region, and provider;
- execution host kind and attach transport;
- repository checkout and sync capability;
- tool allowlist and approval policy;
- child-session limits, timeout limits, and artifact limits;
- verification-pack artifact refs;
- environment compatibility reasons when a Coder deployment cannot run.

Autopilot should dispatch to hosted Probe only when the advertisement satisfies
the Coder deployment's repository, runtime, approval, artifact, and child
session constraints. If matching fails, return a Coder receipt with the
product-safe incompatibility reason.

## Minimal Smoke

A hosted Probe Coder smoke should:

1. send a `StartSession` request with Coder job/deployment metadata and an
   idempotency key;
2. receive an accepted managed session ref and transcript artifact ref;
3. stream a `probe.website_event.v1` batch with `run_started`, at least one
   progress or tool event, optional approval, artifact ref, and terminal event;
4. replay the same events after a cursor;
5. resolve one approval if present;
6. cancel or interrupt the smoke session and receive a terminal projection;
7. verify that website-safe events contain no provider tokens, local paths, raw
   tool payloads, or unbounded output;
8. attach the redacted event batch and verification-pack refs to the Autopilot
   Coder receipt.

The fixture-backed protocol test in
`crates/probe-protocol/tests/coder_runtime_adapter_contract.rs` is the current
minimal smoke path. A live worker smoke can replace the fixture once hosted
Probe is selected as the active Coder runtime worker host.

## Current Missing Fields

No blocking managed-runtime fields are missing for the Autopilot Coder MVP.
Autopilot can carry Coder-specific ids in `ManagedRuntimeCorrelation` and
`ManagedSessionStartRequest.metadata`. If future Coder receipts need richer
fields, add them as product-safe metadata first and promote them into typed
Probe protocol only after a second consumer needs the same field.
