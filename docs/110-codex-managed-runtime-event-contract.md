# Codex Managed Runtime Event Contract

Status: canonical
Last updated: 2026-06-01
Owner: OpenAgents
Canonical successor: none
Related docs:
- `102-codex-vm-workroom-adapter-contract.md`
- `103-signature-context-contract.md`
- `106-codex-signature-attachment.md`
- `107-signature-promotion-and-failure-contributions.md`
- `../../vortex/docs/2026-06-01-long-running-shc-codex-delegation-audit.md`
- `../../vortex/docs/2026-06-01-account-backed-shc-training-loop-audit.md`
- `../../cloud/docs/control/CODEX_CONTROL_API.md`

## Purpose

Probe needs a detailed event stream for account-backed Codex workrooms that run
on SHC or another Cloud backend. The compact `probe.codex_workroom.v1`
lifecycle remains useful for status and closeout. The richer stream below is
the contract for replay, Vortex timeline projection, benchmark traces,
failure learning, continuation, and signature improvement.

The implemented schema is:

```text
probe.codex_managed_event.v1
```

This is still a Probe runtime contract. Codex packages or SKILL.md files are
adapters generated from Blueprint/Probe signatures; they are not the canonical
product object and cannot promote themselves.

## Event Vocabulary

The Cloud/SHC runner should normalize Codex output into these event types:

| Event type | Meaning |
| --- | --- |
| `run_queued` | Assignment accepted but not running yet. |
| `run_started` | Runner started Codex with an account-backed auth grant. |
| `run_heartbeat` | Runner is alive and still owns the session. |
| `turn_started` | A prompt/turn entered Codex. |
| `message_delta` | Assistant text streamed or observed. |
| `message_completed` | Assistant message completed. |
| `tool_call_started` | Codex requested or began a tool action. |
| `tool_call_delta` | Tool arguments or output changed. |
| `tool_call_completed` | Tool result was observed. |
| `shell_command_started` | Shell command began, with command digest/summary. |
| `shell_output_delta` | Shell stdout/stderr chunk. |
| `shell_command_completed` | Shell command exit and duration. |
| `file_edit` | File create/update/delete or patch summary. |
| `artifact_created` | Transcript, diff, log, result, verifier output, or proof ref. |
| `receipt_created` | Closeout, execution, grading, or usage receipt ref. |
| `resource_usage_captured` | Host/device/workspace/resource usage was captured, usually with an `openagents.resource_usage_receipt.v1` digest. |
| `model_usage_reported` | A provider, selector, verifier, judge, or other model call reported token/cost usage. |
| `usage_unavailable` | Token/cost usage was unavailable and why. |
| `run_waiting_for_input` | Runner needs follow-up input or approval. |
| `failure_classified` | Probe/runner classified a failure fingerprint. |
| `continuation_checkpoint` | Durable cursor for replay/resume. |
| `signature_pack_selected` | Probe selected Blueprint/Probe signatures. |
| `codex_package_rendered` | A Codex adapter package was rendered from signatures. |
| `codex_package_validated` | The adapter package passed validation. |
| `codex_package_loaded` | Codex was launched with the package context. |
| `redacted` | Content was intentionally removed. |
| `run_failed` | Terminal failure. |
| `run_timed_out` | Terminal timeout. |
| `run_cancelled` | Terminal cancellation. |
| `run_completed` | Terminal completion, not acceptance. |

Cloud's runner may emit dotted names such as `tool.call.started` or
`shell.output.delta`; Probe normalizes those into the snake-case enum above.
`resource.usage.captured` and `model.usage.reported` are normalized the same
way.

## Usage And Resource Evidence

Model usage and machine resource usage are separate event families.

`model_usage_reported` is for model-call accounting that a backend or Probe
component actually knows. Its typed payload can record:

- `provider`, `backend`, `model`, and `mode`
- account and grant refs, never raw credentials
- input, cached-input, output, reasoning, tool/function-call, and total tokens
- `countSource`: `provider_reported`, `codex_reported`,
  `parsed_from_stream`, `estimated`, or `unavailable`
- optional `costMicrousd` and billing basis

Probe should use this event for selector/planner calls, signature rendering or
validation calls that use a model, oracle calls, verifier/judge calls, and any
Codex backend call that exposes token usage.

`resource_usage_captured` is for host/workroom facts such as provider lane,
node ref, sandbox profile, workspace digest, wall time, exit status,
workspace/artifact/log byte counts, KVM availability, and Firecracker
candidacy. On Cloud/SHC it should cite an
`openagents.resource_usage_receipt.v1` receipt digest through `receiptRefs`;
Probe preserves digest-only Cloud refs as receipt digests so Vortex can join
the event to the durable receipt.

`usage_unavailable` is not a missing record. It is an explicit accounting fact.
For subscription-backed ChatGPT/Codex runs where the CLI does not expose token
counts, the event must include the provider/backend/model/mode when known,
`countSource=unavailable`, a reason such as
`codex_subscription_usage_not_reported`, and the same usage receipt digest as
the paired `resource_usage_captured` event.

Telemetry payload redaction is field-aware: numeric token and cost fields are
not erased merely because their keys contain `token`, but all string values
still pass through the Codex workroom redactor.

## Required Event Shape

Every event carries:

| Field | Meaning |
| --- | --- |
| `schemaVersion` | `probe.codex_managed_event.v1`. |
| `sequence` | Monotonic per run/session sequence. |
| `occurredAtMs` | Runner event time in milliseconds. |
| `eventType` | Normalized event enum. |
| `runRef` | `workroomId`, `runId`, `sessionId`, optional `threadId`, `turnId`, `taskRef`. |
| `retentionMode` | `retained`, `metadata_only`, or `local_only`. |
| `trainingUse` | `allowed`, `denied`, or `needs_review`. |
| `dataRightsRef` | Optional policy/owner ref used by Vortex. |
| `artifactRefs` | Durable refs with digests/redaction state. |
| `receiptRefs` | Closeout, grading, execution, usage, or settlement refs. |
| `payload` | Typed payload for the event kind. |
| `redacted` | True when content was scrubbed or local-only metadata was stored. |

## Signature And Package Evidence

Signature events must make the signature-to-Codex adapter boundary visible:

```text
signature_pack_selected
  -> selected Blueprint/Probe signature ids
  -> optional SessionSignatureContext

codex_package_rendered
  -> package id
  -> adapter kind
  -> rendered ref and digest
  -> source signature ids

codex_package_validated
  -> validation status
  -> required evidence refs

codex_package_loaded
  -> loaded timestamp
  -> package digest
```

This lets Vortex later compare raw Codex, selected-signature Codex, and
candidate-signature reruns without treating Codex's local package as the
source of truth.

## Retention And Opt-Out

Default mode is `retained` because OpenAgents needs durable traces for replay,
debugging, benchmark proof, signature learning, and future compensation.

If a user or organization chooses `local_only`, content-bearing event payloads
must not be promoted into shared Convex state or shared learning. Probe keeps
only minimal event metadata and local refs. Local-only data cannot create
shared signature proposals, marketplace attribution, public benchmark proof, or
paid-workflow compensation claims unless the owner later promotes redacted
evidence.

`metadata_only` allows operational projection without transcripts, diffs, or
tool output.

## Redaction Requirements

Runners and Probe must redact before persistence:

- raw ChatGPT/Codex tokens and `auth.json`;
- API keys and bearer tokens;
- GCP credentials and local `.secrets` paths;
- local user home paths;
- private environment values.

The test fixtures cover those shapes. Vortex should reject content-bearing
callbacks that are not already redacted or explicitly retained under the
accepted policy.

## Continuation

Continuation is sequence-based:

```text
continuation_checkpoint
  checkpointRef
  afterSequence
  resumeHint
```

`resumeHint` may name a Codex thread or local checkpoint class, but must not
include raw auth material. Vortex can replay events after a sequence and ask
Cloud/Probe to resume from the checkpoint through an API, not by reusing this
laptop as orchestrator.

## Validation

The protocol tests include:

- conversion from a representative SHC/Cloud runner JSONL event log;
- redaction of Codex auth, provider credentials, and local paths;
- reported model usage, explicit unavailable usage, and resource usage receipt
  digest joins;
- signature package evidence with source signature ids and fixture refs;
- failure-learning payloads that can carry
  `probe.signature_failure_learning.v1` findings and candidate Blueprint/Probe
  signature revision proposals for retained failures;
- local-only retention that stores metadata instead of content.
