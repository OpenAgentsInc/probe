# Signature Selector

Status: implementation contract

Date: 2026-06-01

Related:

- `docs/103-signature-context-contract.md`
- `docs/104-seed-signature-packs.md`
- `docs/108-adaptive-signature-budget-and-rendering.md`
- `../crates/probe-core/src/signature_registry.rs`

## Purpose

Probe now has a local selector that turns a typed `TaskEnvelope` into a
`SessionSignatureContext` before a Codex-backed session starts.

The selector is intentionally central and typed. It does not scatter ad hoc
keyword routes through runtime code.

## Task Envelope Inputs

`TaskEnvelope` carries:

- visible instruction text;
- dataset slug/version and task id;
- repo URL/ref, languages, and package managers;
- visible manifests;
- expected artifacts;
- verifier command;
- failure fingerprints;
- tool and network policy labels;
- data class;
- scenario tags.

## Selection Model

The MVP selector combines:

- structured boosts for benchmark family, exact fixture id, declared failure
  fingerprint, task class, and expected artifact overlap;
- deterministic local cosine scoring over normalized task/signature documents;
- a minimum score threshold;
- a max-signature cap;
- adaptive budget modes;
- runner-up preservation for later adaptive-budget learning.

No external embedding service is required for the unit-test path. The API can
later swap in durable embeddings or a learned utility model without changing
the `SignatureSelectionDecision` output contract.

## Output

The selector returns a `SessionSignatureContext` with:

- selected signature entries;
- `SignatureSelectionDecision`;
- selected and runner-up scores;
- selected budget and budget mode;
- rejected high-score signatures excluded by cap or budget;
- rendered task-local context;
- task-envelope digest;
- selector mode: `hybrid` or `no_match`;
- fallback reason for no-match cases.

Generic greetings, account-login prompts, and unrelated CRM/auth prompts should
produce an explicit no-match decision.

## Guardrails

- The selector does not grant tool authority.
- The selector does not promote signatures.
- The selector does not make public benchmark claims.
- The selector must preserve runner-up scores when the cap excludes plausible
  candidates.
- Full-injection is blocked by default and is available only as an explicit
  controlled-evaluation baseline.

## Validation

The scenario tests cover:

- service tasks select `coding.service_readiness`;
- PyPI/simple-index tasks select `coding.python_package_index`;
- legal deliverable tasks select `legal.deliverable_file_workflow` and
  `legal.output_path_contract`;
- greetings and ChatGPT-account/login prompts select no signatures;
- candidate-heavy envelopes respect the cap and preserve runner-ups;
- no-signature, fixed-top-k, capped-selector, and full-injection baselines are
  available in the retained fixture ablation report;
- co-selected signatures render `Use for`, `Do not use for`, `Required
  evidence`, and `Neighbor boundaries` clauses.
