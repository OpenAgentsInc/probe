# Signature Context Contract

Status: implementation contract

Date: 2026-06-01

Related:

- `docs/99-managed-tool-permission-policy.md`
- `docs/102-codex-vm-workroom-adapter-contract.md`
- `../crates/probe-protocol/src/signature_context.rs`

## Purpose

Probe now has a first protocol-level signature context contract:

```text
probe.signature_context.v1
```

The contract lets a caller attach selected task signatures to a Probe session
without changing Codex or turning signatures into arbitrary executable tools.

The boundary is:

```text
signatures decide context and policy hints
Probe enforces runtime/tool policy
Codex remains one backend
Vortex records product Program Runs and acceptance
```

## Protocol Objects

`SignaturePack` carries the selected signature entries:

- stable `id`
- required `version`
- adoption state: `candidate`, `shadow`, `promoted`, or `deprecated`
- task classes and benchmark families
- required evidence
- recommended tools
- forbidden tools
- failure fingerprints
- fixture refs
- optional rendered description

`SignatureSelectionDecision` carries selector output:

- decision id
- selector mode
- task-envelope digest
- selected signatures
- runner-up signatures
- recommended harness/tool hints
- forbidden tools
- fallback reason code

`SessionSignatureContext` combines the pack and the optional decision. It is
stored in session metadata and accepted on `ManagedSessionStartRequest`.

## Website-Safe Projection

`SessionSignatureContext::website_safe_projection()` intentionally projects a
redacted shape:

- selected ids and versions
- adoption states
- source refs
- rank and score
- reason codes
- task-envelope digest

It does not project raw task envelopes, rendered descriptions, prompts, file
paths, private repo content, shell logs, or customer data.

## Runtime Events

Managed runtime can emit:

```text
ManagedRuntimeEventType::SignatureContextSelected
ManagedRuntimeEventPayload::SignatureContext
```

When a managed session starts with signature context, Probe persists the
signature context in metadata and emits the signature-context event before the
initial turn event.

## Non-Goals

- This does not add executable domain helper tools.
- This does not fork Codex.
- This does not bypass existing tool approval policy.
- This does not make Probe the product acceptance authority.

## Validation

The protocol tests cover:

- JSON round-trip for signature context;
- rejection of unknown adoption states;
- rejection of missing signature versions;
- website-safe projection redaction.
