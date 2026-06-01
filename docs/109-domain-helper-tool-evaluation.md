# Domain Helper Tool Evaluation

Status: implementation contract

Date: 2026-06-01

Related:

- `docs/103-signature-context-contract.md`
- `docs/105-signature-selector.md`
- `docs/106-codex-signature-attachment.md`
- `docs/108-adaptive-signature-budget-and-rendering.md`
- `../crates/probe-core/src/domain_helpers.rs`
- `../crates/probe-core/src/tools.rs`

## Purpose

Probe should not fork Codex or mutate per-turn tool surfaces just because a
signature exists. The default path remains context-pack injection plus the
normal `coding_bootstrap` tool set.

Executable domain helpers are a later, measured escape hatch for retained
failures where context alone is not enough.

## Default Boundary

`ToolRegistry::coding_bootstrap(false, false)` still exposes only:

- `read_file`
- `list_files`
- `code_search`
- `shell`
- `apply_patch`

Domain helpers are available only through the explicit opt-in constructor:

```text
ToolRegistry::coding_bootstrap_with_domain_helpers(..., include_domain_helpers = true)
```

This keeps the normal Codex session path working without helper tools and gives
operators a separate policy surface for managed manifests.

## First Prototype Helper

The first prototype helper is:

```text
legal.inspect_answer_file
```

It is read-only. It inspects a legal benchmark answer file and returns:

- relative path;
- byte count;
- bytes inspected;
- line count;
- file SHA-256;
- required-substring presence flags;
- required-substring SHA-256 values.

It does not return raw answer content or raw required-substring markers. That
shape is intentional for Harvey-style legal retained fixtures, where the agent
often needs to confirm that an answer artifact exists and includes required
legal terms without leaking the answer body into generic tool summaries.

## Managed Manifest Policy

The helper is projected through the existing managed tool manifest with:

- declared risk: `read_only`;
- execution owner: `probe_runtime`;
- bounded input schema;
- bounded result schema;
- redaction summary that states raw legal answer markers are not exposed.

Read-only policy still auto-allows under the conservative approval config.
Write, network, and destructive helpers remain out of scope until trusted
identity, receipts, and approval policy exist.

## Evaluation Report

`domain_helpers.rs` adds a small retained-fixture report:

```text
context_only arm
helper_assisted arm
recommendation
required_next_evidence
```

The recommendation can be:

- `context_only`
- `prototype_helper`
- `keep_disabled_need_more_evidence`

The helper should be enabled outside controlled runs only after retained
fixtures show a concrete pass delta or a material evidence-quality delta over
context-only runs.

## Current Decision

Use context packs first for Terminal-Bench and Harvey retained failures.

Prototype `legal.inspect_answer_file` for controlled Harvey/legal retained
fixtures because it provides a narrow, read-only inspection primitive that
context cannot always replace. Do not add write/deploy/domain mutator tools
yet, and do not fork Codex for per-turn custom tools unless retained evidence
shows this managed manifest approach is insufficient.
