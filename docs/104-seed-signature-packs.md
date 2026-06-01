# Seed Signature Packs

Status: implementation contract

Date: 2026-06-01

Related:

- `docs/103-signature-context-contract.md`
- `docs/107-signature-promotion-and-failure-contributions.md`
- `../crates/probe-core/signature_registry/seed-signatures.json`
- `../crates/probe-core/src/signature_registry.rs`
- `../crates/probe-protocol/src/signature_context.rs`

## Purpose

Probe now has a repo-local seed signature registry:

```text
probe.seed_failure_signatures.v1
```

The seeds are failure-derived candidate signatures for the retained
Terminal-Bench runs and the planned Harvey Legal second lane. They are not
promoted capabilities and they do not prove benchmark improvement until reruns
produce evidence.

## Seed Families

The first coding seeds cover:

- `coding.service_readiness`
- `coding.python_package_index`
- `coding.query_optimizer_workflow`
- `coding.sqlite_wal_recovery`
- `coding.gcode_parser_guard`
- `coding.xss_sanitizer_policy`
- `benchmark.runner_supervisor`

The `coding.sqlite_wal_recovery` seed includes a retained failure lesson from
the SHC Codex rerun: copy the database, WAL, and SHM sidecars before running
`sqlite3` or any library open. SQLite can delete an unreadable WAL during open,
which turns a recoverable WAL task into an unrecoverable base-database-only
attempt.

The first legal seeds cover:

- `legal.deliverable_file_workflow`
- `legal.output_path_contract`
- `legal.source_grounding_trace`
- `legal.citation_provenance_check`
- `legal.answer_integrity_guard`
- `benchmark.legal_judge_supervisor`

Every seed declares task classes, benchmark families, required evidence,
recommended read/check tools, forbidden authority-bearing tool categories,
closeout artifacts, failure fingerprints, and fixture refs.

## Authority Boundary

Seed signatures do not grant write, network, destructive, or secret-bearing
authority. They can recommend read/check tools and evidence obligations, but
Probe's existing tool policy remains the authority gate.

The validator rejects promoted seed signatures and rejects recommended tools
whose names imply write, network, destructive, patch, or secret authority.

Failed runs can create candidate proposal records through the signature
contribution flow, but those proposals remain separate from the seed registry
until fixture evidence, retained-run evidence, and review acceptance admit them
into shadow or promoted routing.

## CLI

List and validate the seed registry:

```bash
probe signatures list
probe signatures list --json
probe signatures propose --signature-cases ./signature_cases --output ./signature_contributions.json
```

The non-JSON output is for quick operator inspection. The JSON output is the
stable handoff shape for Vortex and future registry-generation work.

## Validation

The registry tests cover:

- required Terminal-Bench and Harvey ids;
- candidate/shadow adoption states;
- required evidence and closeout artifact counts;
- no authority-bearing recommended tools;
- building a `SessionSignatureContext` from selected seed ids.
