# Codex Signature Attachment

Status: implementation contract

Date: 2026-06-01

Related:

- `docs/103-signature-context-contract.md`
- `docs/104-seed-signature-packs.md`
- `docs/105-signature-selector.md`
- `../crates/probe-core/src/harness.rs`
- `../crates/probe-core/src/runtime.rs`
- `../crates/probe-core/src/dataset_export.rs`

## Purpose

Probe can now start a Codex-backed coding session with a selected signature
pack without forking Codex or injecting arbitrary per-turn tools.

The runtime path is:

```text
TaskEnvelope
  -> Signature selector
  -> SessionSignatureContext
  -> coding_bootstrap_codex@v1 harness prompt addendum
  -> normal Probe tool loop and approval policy
  -> transcript note and dataset export refs
```

## Runtime Behavior

`PlainTextExecRequest`, `StartSessionRequest`, and
`ManagedSessionStartRequest` can carry `SessionSignatureContext`.

When present, Probe:

- persists the context on `SessionMetadata`;
- appends a compact `Probe Signature Addendum` to the system prompt;
- records a transcript note containing the decision id, selected signatures,
  recommended tool choice, actual Probe tool choice, and `tool_policy=probe_enforced`;
- emits the managed-runtime `SignatureContextSelected` event on the managed
  session path;
- preserves signature-selection refs in replay, decision, and decision-case
  dataset exports.

## Authority Boundary

The addendum is advisory context. It can tell the model what failure-derived
workflow to use, what evidence to produce, and which tools are recommended or
forbidden by the selected signature.

It does not:

- grant tool access;
- bypass approvals;
- make signatures executable helper tools;
- modify Codex internals;
- override workroom, dataset, or product acceptance policy.

Probe still owns the tool registry, tool choice, approvals, transcript, and
evidence capture. Codex is still one backend behind the Probe runtime.

## Dataset Exports

Exports include compact signature refs:

- `pack_id`
- `selected_signature_ids`
- `decision_id`
- `selector_mode`
- `task_envelope_digest`

This is enough for replay, failure learning, and promotion analysis without
leaking raw task envelopes or private customer data.

## Validation

The targeted smoke path covers:

- prompt addendum rendering;
- Codex-harness session metadata persistence;
- transcript decision/tool-policy note;
- exported signature-selection refs;
- protocol version bump for runtime API compatibility.
