# Probe zerobase — audit of the current tree and spec for the Rust-core architecture

Date: 2026-08-18
Status: full audit + salvage ledger + architecture spec + execution plan. Authorizes the deletion of nearly the entire current tree; ships no runtime change itself.
Supersedes: the reset framing in `README.md` and all `docs/probe-blueprint-*.md`, `docs/probe-apple-fm-*.md`, and benchmark-lane docs.
Companions (in the sarah repo): `docs/audits/2026-08-18-fx-embeddable-coding-agent-audit.md`, `docs/audits/2026-08-18-owned-embeddable-coding-agent-audit.md`.

The owner's direction:

> "i want to essentially delete all of whats there (salvaging anything
> relevant) but zerobase on this new architecture."

The new architecture, established in the companion audits: **one sans-I/O
Rust agent core, compiled both native and to WebAssembly, wrapped by
Effect/TypeScript host packages, speaking Agent Client Protocol (ACP) v1
as a server, with pluggable model transports** — a Sarah-minted inference
grant against Sarah's provider proxy, a local OpenAI-compatible endpoint
(Psionic), and direct API keys — launched on user machines by
`sarah-computer-controller` and later deployable into managed sandboxes.
Probe is the designated first-party OpenAgents coding agent runtime; this
document is the contract for rebuilding it as exactly that.

---

## Part 1 — What is actually in the tree today

This is the third Probe. The first was a 13-crate Rust workspace
(~123,000 lines) amputated in one commit (`92134ae`, 2026-06-07,
"Archive probe for first-party runtime refactor"). The second is the
current TypeScript/Effect/Bun tree: ~200 commits since the reset,
**15,281 source lines** across 53 files in one workspace package,
**6,958 test lines** across 37 files, **~6,050 doc lines** across 31
top-level docs. `README.md` still says "`main` is intentionally clean
while the refactor begins" and lists eight tracked files; the tree it
describes no longer exists.

An honest accounting of what those 15k lines are:

**Three dead lanes make up over 60% of the source.**

- `blueprint/` (2,669 LOC + ~1,100 test LOC + 7 docs): a local mirror of
  the Omega-side Blueprint program-signature registry — signature
  families, risk classes, release gates, a contribution workflow.
  Blueprint is deprecated workspace-wide; this is its last active
  consumer in any repo. It is also load-bearing: eleven non-blueprint
  files import it, including `contracts/assignment.ts` (embeds a
  `ProbeBlueprintAssignmentScope`) and `runner/identity.ts` (calls
  `validateProbeAssignmentBlueprintScope` inside the authorization gate).
  Deleting it is surgery, not `rm -rf`.
- `backends/apple-fm/` (~2,427 LOC + ~1,220 test LOC + 3 docs): a client
  for an Apple Foundation Models HTTP bridge that **does not live in this
  repo** (`http://127.0.0.1:11435`), with snapshot-mode streaming (the
  bridge re-sends the whole assistant text each tick) and an inverted
  tool model where Probe runs a local HTTP callback server the bridge
  calls back into. The architecture being replaced, squared.
- `benchmark/` + `contracts/benchmark.ts` + most of `fleet/` (~3,600
  LOC): schema adapters for GEPA candidate manifests owned by Psionic
  (`psionic.probe_gepa_candidate_manifest.v1`) and Benchmark Cloud
  (`benchmark_cloud.probe_candidate_import.v1`), plus a closeout-bundle
  writer. The optimizer itself never lived here; this is ingest/writeout
  shell for external contracts that have moved on.

**The interactive surface is a monolith with two known-broken seams.**

- `cli.ts` is **1,974 lines in one file** (~57 functions): hand-rolled
  arg parsing, an ANSI Markdown renderer, tool implementations,
  per-backend formatters, and two chat loops with no session persistence.
  It contains dead code (`writeAnyWorkspaceFile` at `cli.ts:1251`,
  superseded and never called).
- `permission.ts` **does not gate anything.** The default handler is
  `ask: () => Effect.succeed("allow")` and `setPermissionHandler` is
  never called anywhere in src or tests. Its own header comment states
  the architectural blocker honestly: tools run in a forked Effect fiber
  while the chat loop owns stdin, so a permission prompt cannot do
  synchronous stdin I/O from inside a tool handler. Commits `29459f1`
  and `cfdd422` are two failed attempts at fixing this in-process.
- `file-mutation.ts` (439 LOC, commit `efcb799`) advertises "BOM,
  line-ending, locking, stale-content guard, and permission gating." The
  primitives are correct in isolation, but the integration has three
  defects: (1) **the stale-content guard is inert** — the
  `StaleContentError` is swallowed by a blanket
  `Effect.catch((error) => Effect.succeed(void 0))` and the edit reports
  success, so a detected concurrent modification is silently dropped;
  the same swallow makes genuine write failures (EACCES, ENOSPC) report
  success; (2) `applyAnyWorkspaceFilePatch` has none of the guards and
  parses a hand-invented patch format that is not OpenAI `apply_patch`;
  (3) the permission gating is the always-allow stub above. It has
  **zero test coverage**, as do `permission.ts`, `opentui-renderer.ts`,
  and effectively `cli.ts` (97 test lines against 1,974).
- Probe has **never had a real shell tool**. The name `shell` is reserved
  in the Apple FM enum; the executor is a noop mock
  (`docs/probe-shell-opencode-parity-audit.md` says so plainly).

**Infrastructure gaps.** There is no `tsconfig.json` anywhere — no
typecheck step exists; correctness rests entirely on `bun test`. Effect
is pinned to a beta (`4.0.0-beta.70`) with recorded API churn
(`1623513`). `marked` is a declared dependency with no import. The
`packages/*` workspace glob has exactly one package.

**And the decisive absence:** there is **zero mention of ACP** anywhere
in src, docs, or README. Nothing in this tree implements, partially
implements, or even references the protocol the new architecture is
built on. The rebuild is genuinely greenfield at the protocol layer.

The pattern across all of it: every bug found in this audit sits in code
with no tests (file-mutation, permission, cli), and every subsystem worth
keeping has dense fixture tests (llm core, Gemini protocol,
materializer). That correlation is a lesson, recorded in Part 3.

---

## Part 2 — Salvage ledger

The bar for salvage: the new architecture would independently need it,
and the existing version is correct. Three subsystems clear the bar
wholesale, a handful of pure functions and vocabularies clear it in
extract form, and four docs survive as requirements. Everything else is
deleted.

### Keep wholesale

**`src/llm/` (518 LOC) — the sans-I/O contract, already written.**
A provider-neutral LLM core in Effect Schema: content-part union
(text / media / reasoning / tool-call / tool-result) with cache hints
and `providerMetadata` escape hatches; `ProbeLlmRequest`; a flat tagged
streaming event union
(`step-start | text-delta | reasoning-delta | tool-call | tool-result |
tool-error | provider-error | step-finish | finish`) with id-keyed
deltas so interleaved streams reassemble; normalized usage that clamps
`reasoningTokens` to `outputTokens`. Fully immutable, no I/O, imports
only `effect`. The dispatcher emits both a `tool-error` and a
`tool-result` so a strict consumer never sees an unpaired call. **This
is the Rust core's type system, pre-designed.** Port the seven schemas
to Rust as the canonical definition; keep the TS versions as the
host-side mirror; `llm-core.test.ts` (158) pins the contract.

**The Omega credential subsystem (~1,460 LOC + ~700 test LOC) — the
best-engineered code in the repo.** Branded refs
(`ProviderAccountRef` / `ProviderAuthGrantRef` / `ProviderSecretRef` via
`S.brand`) so a secret ref cannot be passed where an account ref is
expected; `grant-client.ts` resolving a grant into a **materialization
plan, not a secret**, with exhaustive cross-checks (provider, refs,
runner session, expiry, status) and six tagged failure modes;
`runner/identity.ts` deriving required capabilities per assignment and
gating grant resolution on a linked runner identity;
`auth/materializer.ts` placing the secret into a per-run env var or
0o600 file inside a path-escape-guarded run home, bracketed with
`Effect.acquireUseRelease` so the scrub runs on success, failure, *and*
interruption, with every receipt validated through
`validateProbePublicProjection` and carrying `contentRedacted:
S.Literal(true)` — redaction enforced by the type system. This is
I/O-and-lifecycle code; it stays in the Effect/TS host layer, and it is
**structurally the client half of Sarah's inference grant**: grant →
plan → materialize → scrub is exactly the lifecycle
`sarah.inference_grant.v1` needs. Strip the Blueprint scope from
`contracts/assignment.ts` as part of the salvage. Keep
`materializer.test.ts`, `grant-client.test.ts`, `runner-identity.test.ts`
(~700 lines of failure-path coverage). Known minor gap: file scrub
unlinks without overwriting.

**The Gemini wire-protocol layer and its test corpus (~500 LOC + ~670
test LOC).** `protocol.ts` is a bidirectional, sans-I/O lowering between
the neutral LLM contract and Gemini's `contents`/`parts` shape, plus an
incremental SSE parser with explicit parse state
(`makeGeminiSseParseState` / `parseGeminiSsePayload` /
`finishGeminiSseParseState`) that handles `thought`/`thoughtSignature`
reasoning parts and split-chunk boundaries. `gemini-protocol.test.ts`,
`gemini-stream-parser.test.ts`, `gemini-fixture-coverage.test.ts`, and
`gemini-tool-loop.test.ts` are real wire-format fixtures — **a
conformance suite for whatever the Rust core's parser becomes.** Port
the lowering and parse-state design to Rust; port the fixtures verbatim.

### Extract, then delete the husk

- From `file-mutation.ts` (~60 of 439 lines): the pure functions —
  `splitBom`/`joinBom`/`hasUtf8Bom`, `detectLineEnding`/
  `convertToLineEnding` (CRLF preserved through an edit),
  `countOccurrences`, and the exact-match edit policy (0 matches →
  error; >1 without `replaceAll` → "provide more context"). These belong
  in the Rust core as pure logic. The Effect wrappers, the inert guard
  plumbing, and the invented patch parser die.
- From `workspace.ts`: the `resolveWorkspacePath` containment check
  (rejects `..`, NUL, `.git`) as a concept; the root-sniffing that
  hardcodes `packages/runtime/src/cli.ts` dies.
- From `backend-profile.ts`/`registry.ts` (45/136): the **resolved
  profile with `baseUrlSource` provenance** ("explicit" | env-var name |
  "default") — an auditable record of *why* a backend URL is what it is.
  The hardcoded two-profile registry dies.
- Vocabularies (~40 lines total): the `ProbeAuthFailureClass` taxonomy
  from `fleet/telemetry.ts` (`requires_reauth`, `low_credit`,
  `rate_limited`, …) and the **usage-truth tri-state**
  (`exact | estimated | unknown`) from the Apple FM contract — hard-won
  words, cheap to keep.
- From `blueprint/` (~30 lines of concept in 2,669 of machinery): the
  tool-menu policy triple `allow | approval_required | deny` derived
  from a declared scope, and the effect-kind classification separating
  local sandbox read/edit from external irreversible effects. Both feed
  the ACP permission model. The registry dies.
- From `cli.ts` (a pattern, not code): the escape-to-interrupt design —
  completion in a forked fiber, raw-mode stdin during streaming,
  double-Escape in a 5s window, `fiber.interruptUnsafe()` resolved
  through `addObserver` with proper `Cause` discrimination. Correct
  fiber hygiene worth reproducing in the new host CLI.

### Keep as requirements documents

- `docs/probe-write-edit-tool-opencode-parity.md` (474)
- `docs/probe-rendering-gap-audit.md` (531)
- `docs/probe-shell-opencode-parity-audit.md` (187)
- `docs/2026-06-08-gemini-opencode-support-audit.md` (474)

These four are gap analyses against a mature reference implementation
and read as specifications for work never done. They are the tool-layer
requirements input for Phase 4 below. Also retained: the six Omega
auth-contract docs (Theme A) alongside the salvaged subsystem — with the
recorded caveat that the Omega server half ("provider-account
storage/grant issuance remains a follow-up") was never finished, and
`docs/probe-llm-core.md` + `docs/probe-gemini-backend.md` +
`docs/probe-token-usage-telemetry.md` describing kept contracts.

### Delete

| What | ~LOC (src+tests+docs) | Why |
| --- | --- | --- |
| `blueprint/` + 7 tests + 8 docs | ~4,700 | Deprecated system, last active consumer, fixture-driven mirror of a registry that isn't authoritative |
| `backends/apple-fm/` + 8 tests + 3 docs | ~4,600 | External sidecar dependency, snapshot streaming, reverse-callback tool server — each individually superseded |
| `benchmark/` + `contracts/benchmark.ts` + 5 tests + 4 docs | ~3,600 | Adapters for external Psionic/Benchmark-Cloud/Pylon schemas we don't control |
| `cli.ts`, `permission.ts`, `opentui-renderer.ts` + `cli.test.ts` | ~2,270 | Monolith; non-functional permission gate; 3-commit-old renderer wrapper that is 90% a color table |
| `fleet/backend-capability.ts`, `fleet/token-usage.ts`, `fleet/telemetry.ts` (after vocabulary extraction) | ~1,300 | Doubly coupled to Blueprint and Apple FM |
| `file-mutation.ts` husk, `workspace.ts` husk | ~380 | Defective integrations around salvaged pure functions |
| `docs/playground.md` (untracked) | 40 | A nonsense-Markdown renderer fixture ("The Quantum Wobulator"), not a doc |
| Stale `dist/`, `var/` on disk | — | Produced by no script |

Net: of 15,281 source lines, roughly **2,500 survive** (llm, Omega
subsystem, Gemini protocol) plus ~200 lines of extracts and
vocabularies; of 6,958 test lines, roughly **2,000 survive** (llm,
gemini, credential-model corpora).

---

## Part 3 — What the two dead Probes teach the third

**Lesson 1 — permission gating must be a protocol, not a prompt.** The
TS tree's permission stub is the clearest possible proof: an in-process
CLI whose chat loop owns stdin cannot gate tools running in forked
fibers, and two commits of trying could not fix it. ACP's
`session/request_permission` is a request/response over the wire — the
client (controller, Zed, Sarah's delegation lane) owns the interaction
surface and answers from *its* policy. In the new architecture the core
never prompts anyone: it emits a typed permission request as data and
suspends the tool until a decision event arrives. This is a first-class
core concern from Phase 1, not a bolt-on.

**Lesson 2 — type-enforced redaction works; reproduce it.** The
`contentRedacted: S.Literal(true)` + `validateProbePublicProjection`
discipline made "no secrets across this boundary" a compile-time and
validation-time property instead of a convention. The Rust core gets the
same move: receipt/event types whose constructors cannot be built from
secret-bearing values, enforced where the host boundary is crossed.

**Lesson 3 — test density predicted every verdict in this audit.** The
subsystems kept (llm, Gemini protocol, materializer) are exactly the
ones with dense fixture tests; every bug found lives in untested code.
The rebuild rule: **no tool or mutation code merges without fixture
tests**, and wire parsers land with conformance corpora before they land
with transports.

**Lesson 4 — the first Rust tree already solved problems the rebuild
will face.** The `92134ae` amputation preserved, in history: a dedicated
`probe-protocol` wire crate; `probe-server` with a **`stdio_protocol`
test suite** (a stdio-framed agent server is structurally ACP-over-stdio
before ACP existed); a provider-per-crate layout (`probe-provider-openai`,
`probe-provider-apple-fm`, `probe-openai-auth`) that is the
pluggable-transport idea in crate form; `probe-daemon` lifecycle tests;
and a CLI regression `.snap` corpus (521-line acceptance snapshots). The
README explicitly sanctions this archaeology. Before writing the new
crates, read:

```sh
git show 92134ae^:crates/probe-protocol/src/lib.rs
git show 92134ae^:crates/probe-server/tests/stdio_protocol.rs
git show 92134ae^ --stat            # the full 273-file inventory
```

Do not revive it as a compatibility layer (the README's own rule); mine
the framing, session-lifecycle, and failure-case decisions.

**Lesson 5 — one clean amputation beats incremental decay.** `92134ae`
removed 123,309 lines in one commit and left a truthful README. The TS
tree then drifted for 200 commits past its own reset story. The zerobase
below repeats the amputation pattern — one archive commit, one honest
README — and adds the thing the last reset lacked: this spec, committed
first, so the new tree grows against a written contract.

---

## Part 4 — The zerobase architecture

### Thesis

One sans-I/O Rust core, many hosts. The core is a state machine: bytes
and events in, events and effect-requests out. Everything that touches
the world — network, filesystem, processes, clocks, credentials,
terminals — lives in a host. The first host is Effect/TypeScript; the
native binary and the wasm build are the same core under different
entry points. This is the fx host-boundary idea executed with a
toolchain that is good at it, pointed at OpenAgents infrastructure
instead of a vendor gateway.

### Workspace layout

```
probe/
  Cargo.toml                 # Rust workspace
  crates/
    probe-core/              # sans-I/O agent core (the product)
    probe-acp/               # ACP v1 types + framing, sans-I/O
    probe-wire/              # provider lowerings + stream parsers, sans-I/O
    probe-bin/               # native binary: acp server, cli entry
    probe-wasm/              # wasm-bindgen surface over probe-core
  package.json               # Bun workspace (runtime-neutral published output)
  packages/
    host/                    # @openagentsinc/probe-host: Effect services (I/O)
    probe/                   # @openagentsinc/probe: npm package (wasm + host + types)
  docs/
```

Bun remains the repo's JS runtime (existing choice, catalog pinning);
the published npm package must be runtime-neutral (Node LTS, Bun,
browser) — consumers include the pnpm-based `sarah-computer-controller`.
A `tsconfig.json` and a typecheck script land in the first TS commit;
the no-typecheck era ends with the old tree.

### `probe-core` — what the core is

- **The contract types**, ported from `src/llm/`: messages
  (content-part union with reasoning and cache hints), requests, the
  flat streaming event union with id-keyed deltas, tools, normalized
  usage with the tri-state usage-truth. Rust is canonical; the TS mirror
  in `packages/probe` is generated or conformance-tested against shared
  JSON fixtures.
- **The agent loop as a state machine**: turn assembly, bounded
  multi-step tool loops, tool-call parsing, interleave reassembly,
  cancellation as a state transition. No async runtime dependency in the
  core's logic — the host drives it.
- **Permission as data** (Lesson 1): a tool invocation that requires
  consent yields a typed `PermissionRequest` (tool, args digest, effect
  class from the salvaged `allow | approval_required | deny` +
  effect-kind vocabulary); execution resumes only on a
  `PermissionDecision` event. The core never sees a TTY.
- **Edit policy as pure logic**: the salvaged BOM/line-ending/exact-match
  functions, with the stale-content guard rebuilt so that a detected
  conflict is a **typed failure the caller must handle** — the inert
  `catch`-and-report-success defect is the anti-pattern this replaces.
- **Redaction-typed receipts** (Lesson 2).

What the core must **not** contain: sockets, files, processes, clocks
(injected), provider URLs, credentials, rendering, or any dependency
that breaks `wasm32-unknown-unknown`.

### `probe-acp` — the primary product surface

ACP v1 as a *server*: `initialize`, `session/new|load|resume|cancel`,
`session/prompt`, `session/update` notifications,
`session/request_permission`. Sans-I/O: the crate produces and consumes
framed JSON-RPC 2.0 messages; `probe-bin` moves them over stdio,
`probe-wasm` hands them to the JS host. The neutral event union maps
nearly 1:1 onto ACP session updates — that mapping is a pure function
in this crate with its own fixture suite. The controller
(`sarah-computer-controller`'s `AcpAgent.ts`/`AgentCatalog.ts`) is the
first client; Zed-class editors and Sarah's `computer_agent.v1` lane
follow for free.

### `probe-wire` — transports lowered, not owned

A `ModelTransport` is two halves. The **lowering** (request → provider
wire shape, provider stream → neutral events) is pure and lives here:
the Gemini lowering and SSE parse-state design port directly, with the
existing fixture corpus as the conformance suite; an OpenAI-compatible
lowering (chat completions + SSE) is the second implementation and
covers both Psionic-served local models and Sarah's provider proxy. The
**I/O half** (actual HTTP, retries, abort) lives in the host. Planned
transports, in order:

1. **OpenAI-compatible** — one lowering, three deployments: Sarah's
   inference-grant proxy, local Psionic serving, any vanilla endpoint.
2. **Gemini direct** (ported) — API key or Omega broker base-URL
   rewrite, with `baseUrlSource` provenance kept.
3. Whatever the market demands next; the neutral contract is the
   insulation.

### `packages/host` — the Effect layer

Effect services over the core's effect-requests: HTTP transport
(`fetch`-based, abortable, mapped to core cancellation), filesystem and
process execution for tools, session persistence, and the **salvaged
credential subsystem** — grant client, runner identity, materializer
with its `acquireUseRelease` bracket — retargeted so that the grant
issuer is pluggable: Omega today where it exists, **Sarah's
`sarah.inference_grant.v1`** as the primary path (handed to the process
by the controller at launch over the already-authenticated channel; no
long-lived key on disk). The scrub gains overwrite-before-unlink,
closing the noted gap.

### `packages/probe` — the npm package

wasm-bindgen with an **async, promise-based ABI**
(`wasm-bindgen-futures`) — explicitly not JSPI, so it runs on stable
Node LTS, Bun, and all browsers. Ships the wasm, generated typings, the
Effect wrapper, and bundler/node/web targets. Browser residency remains
possible-not-promised, per the companion audits: the first consumers
are the controller and Node-side hosts.

### Placements (unchanged from the owned-agent audit)

1. Paired machine: controller launches `probe-bin acp` from its agent
   catalog, passes the inference grant, streams progress over ACP into
   Sarah's delegation lane.
2. Local-model lane: same binary, transport pointed at Psionic on the
   machine; code and prompts never leave it.
3. Managed workrooms (openagents Cloud) and Pylon-spawned executors:
   same ACP surface inside real sandboxes; sequenced last.

---

## Part 5 — Execution plan

**Phase 0 — land this spec, then amputate.** Commit this document. Then
one archive commit in the `92134ae` mold: delete everything except this
spec, the four parity docs, the Theme-A/kept docs, and a rewritten
README that states the architecture in ten lines and points here. The
salvaged code is *not* moved in the archive commit — it re-enters in its
new location in later phases, pulled from history (`git show
<archive>^:packages/runtime/src/llm/events.ts` etc.), so the amputation
stays one honest cut and every salvage is a reviewed re-landing, not a
blind copy. `docs/playground.md` (untracked) is simply removed.

**Phase 1 — `probe-core` contracts + conformance.** Port the llm
schemas to Rust; port `llm-core.test.ts` and the Gemini
protocol/stream fixtures as shared JSON conformance fixtures consumed
by both `cargo test` and the future TS mirror. Exit: the event model
round-trips fixtures byte-for-byte.

**Phase 2 — `probe-acp` + `probe-bin`.** ACP types, framing, the
event→session/update mapping, `session/request_permission` wired to the
core's permission-as-data states. Native binary serving ACP over stdio.
Exit: the controller's `AcpAgent.ts` drives a session end-to-end against
a stub transport; `probe` appears in the controller's agent catalog
(change in that repo).

**Phase 3 — transports.** OpenAI-compatible lowering + host HTTP
service; Gemini port with its corpus. Exit: a real prompt→stream→tool
loop against a local OpenAI-compatible server and against Gemini,
fixture-tested without the network, live tests env-gated exactly as the
current `gemini-live-smoke` pattern.

**Phase 4 — tools, for real this time.** Shell execution (the first in
Probe's history), read/edit/write on the salvaged pure policy with the
stale-guard rebuilt as a typed failure, every mutation behind an ACP
permission request. Requirements: the four OpenCode parity docs. Exit
rule from Lesson 3: no tool merges without fixture tests.

**Phase 5 — credentials.** Re-land the grant/materializer subsystem in
`packages/host`, pluggable issuer, Sarah inference-grant client per the
sarah-repo grant spec when it lands. Exit: a controller-launched probe
completes a delegation with a short-lived grant and a verified scrub.

**Phase 6 — `probe-wasm` + npm package.** Async ABI, bundler/node/web
targets, published surface. Exit: the same conformance fixtures pass
through the wasm build in Node LTS and Bun.

Workrooms/Pylon placements follow as consumers of the ACP surface, not
as phases of this repo.

### Explicitly NOT carried forward

- Blueprint, in any form — no signature registry, no scope embedded in
  the assignment contract, no static fixtures.
- The Apple FM bridge client, snapshot streaming, and reverse-callback
  tool serving. If Apple FM returns, it returns as an
  OpenAI-compatible or purpose-built transport behind the neutral
  contract.
- The benchmark/GEPA ingest shell and Benchmark-Cloud/Psionic manifest
  adapters.
- In-process stdin permission prompting, in any form (Lesson 1).
- A build-less, typecheck-less TS package.
- Any long-lived provider credential written to disk outside the
  materializer's bracketed, scrubbed run home.
- Reviving the 2026 Rust tree as a base. It is source material
  (Lesson 4), not a starting point.

---

## Part 6 — The one-paragraph answer

The current tree is 15k lines of which roughly 2,500 deserve to live:
a genuinely excellent provider-neutral LLM contract, the
best-in-workspace grant→materialize→scrub credential subsystem, and a
fixture-backed Gemini wire layer — surrounded by three dead lanes
(Blueprint, Apple FM, benchmark ingest) that are over 60% of the code,
a 1,974-line CLI monolith, a permission gate that always allows and is
wired to nothing, and an edit guard that swallows its own failures; and
it contains zero ACP, meaning nothing here implements the protocol the
new Probe is built on. So: amputate in one commit as was done to the
Rust tree before it, re-land the three keepers in their new homes
(contract types into the Rust core, credential subsystem into the
Effect host, wire fixtures as the conformance corpus), mine the old
Rust `probe-protocol`/`probe-server` stdio archaeology for framing
decisions, and build the third Probe as this spec's five crates and two
packages: a sans-I/O Rust core speaking ACP v1 through native and wasm
entry points, permission as protocol rather than prompt, transports
lowered purely and executed in the host, credentials as short-lived
Sarah-minted grants — one core, many hosts, every seam owned.

---

## Appendix — evidence

Current tree (all paths under `packages/runtime/` unless noted):

- `src/llm/{messages,request,events,tool,tool-runtime,usage,index}.ts`
  (518 LOC) — kept contract; `tests/llm-core.test.ts`.
- `src/omega/grant-client.ts` (277), `src/auth/materializer.ts` (231),
  `src/runner/identity.ts` (180), `src/contracts/provider-account.ts`
  (271), `src/contracts/assignment.ts` (359, Blueprint scope to strip);
  `tests/{materializer,grant-client,runner-identity}.test.ts` (~700).
- `src/backends/gemini/protocol.ts` (496) SSE parse state;
  `tests/gemini-{protocol,stream-parser,fixture-coverage,tool-loop}.test.ts`.
- `src/permission.ts:` default handler `ask: () =>
  Effect.succeed("allow")`; `setPermissionHandler` unreferenced; header
  comment admitting the stdin-ownership blocker; failed fixes `29459f1`,
  `cfdd422`.
- `src/file-mutation.ts`: inert stale guard via
  `Effect.catch((error) => Effect.succeed(void 0))` in
  `editAnyWorkspaceFile`/`writeAnyWorkspaceFile`; unguarded
  `applyAnyWorkspaceFilePatch`; zero tests. Dead code:
  `src/cli.ts:1251` `writeAnyWorkspaceFile`.
- Blueprint coupling: 11 importing files incl. `contracts/assignment.ts`
  (`ProbeBlueprintAssignmentScope`) and `runner/identity.ts`
  (`validateProbeAssignmentBlueprintScope`).
- External schema ownership: `psionic.probe_gepa_candidate_manifest.v1`,
  `benchmark_cloud.probe_candidate_import.v1`,
  `ProbeBenchmarkPrivacyTier = ["local_only","shc_box","pylon_worker",
  "remote_api"]`.
- Manifests: root `package.json` (Bun 1.3.11 workspace, catalog
  `effect 4.0.0-beta.70`, `typescript ^6.0.3` unused, no tsconfig);
  `packages/runtime/package.json` (raw-TS exports, `@opentui/core`,
  `diff`, unused `marked`).

History:

- `92134ae` "Archive probe for first-party runtime refactor"
  (2026-06-07): 273 files, −123,309/+68. Crates of note:
  `probe-protocol`, `probe-server` (`tests/stdio_protocol.rs`),
  `probe-daemon`, `probe-provider-openai`, `probe-provider-apple-fm`,
  `probe-openai-auth`, `probe-cli` (`main.rs` 6,170; `.snap`
  regression corpus), `probe-core` (Forge workers, RLM, harness,
  `dataset_export.rs` 2,191).
- Oldest era: `9703094` scaffold → `87ddd64` Rust workspace →
  `35d782b` OpenAI-compatible provider client → `1ea4edd` Psionic Qwen
  backend profile.

Kept docs: `probe-write-edit-tool-opencode-parity.md`,
`probe-rendering-gap-audit.md`, `probe-shell-opencode-parity-audit.md`,
`2026-06-08-gemini-opencode-support-audit.md`, the six Omega
auth-contract docs, `probe-llm-core.md`, `probe-gemini-backend.md`,
`probe-token-usage-telemetry.md`.

Companion architecture sources (sarah repo):
`docs/audits/2026-08-18-fx-embeddable-coding-agent-audit.md` (fx wasm
host boundary, JSPI constraint, gateway lock-in),
`docs/audits/2026-08-18-owned-embeddable-coding-agent-audit.md`
(inference grant, placements, funnel inversion). ACP reference client:
`sarah-computer-controller` `src/AcpAgent.ts`, `src/AgentCatalog.ts`,
`src/AgentDispatch.ts`.

---

# Addendum (2026-08-18) — controller conformance and the free-agent loop

Written after a source-level read of `sarah-computer-controller` (HEAD
`e3e7413`). The owner's product intent, verbatim in substance: users of
the Sarah interface at OpenAgents.com run the controller, authenticate
with GitHub, get computer control that includes probe alongside the
other coding agents — and because pairing binds the machine to their
account, their delegations hit our API on our credits. **Effectively a
free coding agent with free inference.** This addendum verifies the
Part 4 architecture against the controller as it actually exists,
records the exact conformance contract probe must meet, identifies the
one deliberate contract amendment the loop needs, and amends the phase
order where the reading changed it.

## A1 — The product loop, end to end

1. **GitHub is the identity; pairing is the auth.** The user signs into
   Sarah with GitHub OAuth. The controller pairs device-style
   (`src/Api.ts`): it POSTs to `{endpoint}/controller/pairings`, prints
   a short code, the user approves that code in their
   GitHub-authenticated browser session, and the controller claims a
   **machine token** exactly once — stored 0600 under the controller's
   config dir, never printed, journaled, or passed as an argument
   (`src/Config.ts` `writeToken`). The machine never talks to GitHub;
   GitHub identity lives server-side and the token binds machine →
   account.
2. **Delegation rides the existing channel.** Sarah's
   `computer_agent.v1` sends an `agent` event; `src/AgentDispatch.ts`
   gates it on the machine's tier (probe tier refuses delegation
   outright; `curated` is the product path), resolves the agent id
   against the catalog, and launches a bounded ACP job.
3. **Inference is account credits, not user keys.** Probe's model calls
   go to Sarah's provider proxy under a per-delegation inference grant
   (Part 4 / the sarah-repo grant spec), metered against the account's
   credit balance. "Free" is then a **pricing decision, not an
   architecture decision**: OpenAgents grants credits to
   GitHub-authenticated accounts, and the same grant/budget/receipt
   machinery enforces whatever the subsidy is. Abuse controls are the
   grant's budget (tokens/calls/wall-clock), the delegation bounds
   below, and Sarah-side account admission — all already specified.
4. **The differentiator the catalog makes visible.** Every other agent
   in the catalog needs its own credential on the machine: the pinned
   claude adapter's `authReady` sniffs for `ANTHROPIC_API_KEY` or
   `~/.claude` credential files; operator entries check named env
   opt-ins. **Probe needs no machine credential at all** — authority
   arrives per-delegation with the work. Probe is the only agent whose
   `auth_ready` can be honestly `true` on every freshly paired machine,
   which is precisely what "a free coding agent for every Sarah user"
   requires.

## A2 — Verified conformance contract for `probe-acp`

The controller is an ACP **client** built on the official
`@agentclientprotocol/sdk` (`src/AcpAgent.ts`). Probe's server surface
must satisfy, exactly:

- **Protocol**: JSON-RPC over stdio, newline-delimited;
  `protocolVersion` must equal **1** on initialize or the delegation is
  marked `unavailable`. Advertise `loadSession: true` — the controller
  resumes via `session/load` only when the agent declares it.
- **Client capabilities are minimal and closed**: `fs: false`,
  `terminal: false`, and only `session/request_permission` +
  `session/update` are registered — any other agent→client method gets
  method-not-found. All probe tools are in-process (Part 4 already says
  this; the controller enforces it).
- **Hard bounds, silently enforced**: default 240 s wall clock (request
  may raise to 600 s max), 128 KiB streamed output per surface, and a
  **4 MiB per-message cap where oversize JSON-RPC lines are dropped
  whole** (`boundedLineStream`). A dropped message is invisible to the
  sender, so probe must chunk `tool_call_update` content and cap
  embedded output far below the ceiling — a message that grows past
  4 MiB doesn't error, it vanishes.
- **Session updates are the UI.** The controller's `renderUpdate` turns
  `tool_call` / `tool_call_update` into structured frames the Sarah
  delegation rail renders as collapsible tool cards. Probe must
  populate `toolCallId`, `kind`, `title`, `rawInput.command` (or
  `executable` + `args`), terminal `status` (`completed`/`failed`), and
  text content blocks. `agent_thought_chunk` is dropped — never put
  user-relevant information only in thoughts. `plan` entries render as
  plan notes; emit them.
- **Permission requests meet a tier-aware local policy**
  (`permissionAllowed`): `shell` tier allows anything (one-shot);
  `curated` allows the read-shaped kinds `read|search|fetch|think` plus
  `execute` requests whose **every** `&&`/`||`/`;`/`|`/newline segment
  heads with a program on the machine's `curatedExecute` allowlist
  (git, gh, ls, cat, rg, node, npm, cargo, go, make, mix, …; `cd` only
  as a chain prefix; interpreters and `sudo`/`ssh` excluded). Probe's
  escalation must therefore: put the real command string in
  `rawInput.command`; use the standard kind vocabulary; offer options
  with standard kinds (`allow_once` preferred — the controller picks
  the conservative one-shot and **refuses any option whose
  id/name/kind matches /bypass/i**); and treat a rejection as a typed
  refusal to route around honestly, not an error. Note the tier
  comment's own stance: "delegated agents edit files through their own
  in-process tools, not this shell surface" — in-workspace edits are
  agent-side policy (disclosed via `tool_call` frames), matching the
  pinned claude adapter's behavior; verify parity in Phase 4.
- **Lifecycle**: `session/cancel` must interrupt promptly — on timeout
  or cancel the controller notifies, waits 5 s, then settles; the
  subprocess always gets SIGTERM with SIGKILL 3 s behind. Probe must
  die fast and idempotently. `stopReason` semantics: `refusal` →
  refused, `cancelled` → cancelled/timeout, anything else → completed.
  stderr is drained and ignored; exiting before the prompt completes is
  recorded as `failed`.
- **Auth**: on `session/new` failing with code −32000 the controller
  tries the first advertised auth method **once, non-interactively**.
  Probe with a grant present advertises no auth methods and never hits
  this path; probe with a missing/expired grant fails with a detail
  string naming the grant, not an auth dance.
- **Scrubbing is double-walled.** The controller scrubs everything it
  emits (`scrubSecrets`), but probe self-scrubs too — the grant value
  must be registered with both walls and must never appear in updates,
  stderr, or tool output.

## A3 — Distribution: the pinned-npm slot is the ship vehicle

The catalog (`src/AgentCatalog.ts`) has three sources with deliberate
precedence: operator config argv, **pinned npm dependency** (the claude
adapter ships as `@agentclientprotocol/claude-agent-acp` in the
controller's own `node_modules`, spawned as
`[process.execPath, binPath]` — version-pinned, no PATH, no network),
and the opt-in vendored registry (npx/uvx, or sha256-verified binary
download, off by default).

This resolves a sequencing question from Part 5 in favor of the wasm
package. For probe to be the **default, zero-install** agent on every
paired machine, it must occupy the same pinned-dependency slot:
`@openagentsinc/probe` as a direct controller dependency, spawned from
`node_modules` under the controller's own Node. The wasm core with the
async (non-JSPI) ABI is what makes that a **single platform-neutral
pinned package** — no per-platform binary matrix, no registry download
plumbing, no sha256 table to maintain. Consequences for the plan:

- **Phase 6 (`probe-wasm` + npm) is promoted from polish to ship
  vehicle.** The development loop still runs Phases 2–5 against the
  native `probe-bin` via an operator config entry
  (`agents: { probe: { argv: ["/path/to/probe-bin", "acp"] } }` — works
  today with zero controller changes). But the *product* milestone —
  probe in every Sarah user's catalog with `auth_ready: true` — lands
  when the controller pins the npm package, exactly as it pins the
  claude adapter.
- The **native binary via the registry snapshot** (sha256-verified
  binary distribution) becomes the later performance/footprint path,
  behind the existing `--allow registry-agents` opt-in, not the
  default.

## A4 — The one contract amendment: grant delivery

The controller's env rule is explicit and load-bearing
(`src/Config.ts`): agent env passthrough is "a deliberate, recorded
opt-in per agent — **never a server-supplied value**," and
`agentEnvironment` builds the child env from the scrubbed controller
environment plus those opt-ins only. The machine token itself never
reaches an agent. As written, there is **no channel by which a
per-delegation inference grant can reach probe** — and that is the one
place the free-agent loop requires a deliberate amendment rather than
mere conformance:

- The delegation event (or a controller-side exchange using the machine
  token) carries a **short-lived, delegation-scoped inference grant**;
  `AgentDispatch` injects it into the child env (e.g.
  `PROBE_INFERENCE_GRANT`, `PROBE_INFERENCE_URL`) at spawn, for the
  first-party probe entry.
- This does not repeal the rule — it narrows it: the rule exists so the
  server cannot exfiltrate or plant *operator secrets*; a
  Sarah-minted grant is not an operator secret but per-run authority,
  generation-fenced, budgeted, useless beyond the delegation, and
  journaled as part of the dispatch. The amendment should be recorded
  in the controller repo with exactly that distinction, and the grant
  value added to `scrubSecrets`.
- Env-at-spawn is preferred over a run file (nothing touches disk;
  `spawn` env is already per-launch). The materializer's
  file-with-scrub pattern remains available if a transport ever needs a
  file-shaped credential.

Routing: the amendment lands in `sarah-computer-controller`; the grant
mint/proxy lands in `sarah` per its grant spec; probe consumes both.
None of it blocks Phases 1–4, which run grant-less against direct
transports.

## A5 — Verdict

The Part 4 architecture survives contact with the controller's source
unchanged in structure and sharpened in three particulars: the ACP
conformance contract is now exact (A2) and becomes Phase 2's test
fixture list; the wasm npm package is promoted to ship vehicle because
the pinned-dependency catalog slot is how a default agent actually
reaches every paired machine (A3); and the free-inference loop needs
exactly one deliberate controller amendment — server-supplied,
delegation-scoped grant env for the first-party agent — plus the
Sarah-side grant mint, both already routed (A4). Everything else the
product idea needs — GitHub-anchored pairing, tier policy, bounded
delegation, the delegation rail, credit accounting shapes — already
exists and was read working in the controller and Sarah sources cited
above. The "free coding agent for every Sarah user" is, concretely: pin
`@openagentsinc/probe` beside the claude adapter, mint grants against
account credits, and let `auth_ready: true` do the talking.

Addendum evidence (`sarah-computer-controller`, HEAD `e3e7413`):
`src/AcpAgent.ts` (SDK client, protocolVersion 1, limits
240s/128KiB/4MiB, `boundedLineStream` whole-line drops,
`readShapedPermissionKinds`, `defaultCuratedExecute`,
`commandChainAllowed`, `selectPermissionOption` bypass refusal,
`renderUpdate` tool frames, auth-required retry `-32000`);
`src/AgentCatalog.ts` (three-source catalog, pinned claude adapter via
`[process.execPath, binPath]`, `claudeAuthReady`, `agentEnvironment`,
`acpAgentInventory` with `auth_ready`); `src/AgentDispatch.ts` (tier
gate — probe tier refuses delegation, timeout cap 600s, terminal
payload contract, `agent_not_available` refusals teaching Sarah the
inventory); `src/Api.ts` (device-style pairing: pairing id + short code
+ poll secret → one-shot machine-token claim); `src/Config.ts` (machine
owns tier/roots, token 0600 never printed, env passthrough "never a
server-supplied value", `curatedExecute` owner opt-in, default endpoint
`https://stage.openagents.com`).
