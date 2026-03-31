# Psionic Qwen Hermes Deep Dive And Probe CLI Roadmap

## Scope

This document explains the prior Hermes and Qwen work that already exists in
`psionic`, and translates that work into the first concrete Probe CLI roadmap.

The practical question is:

- what `psionic` already proved
- what remained in Hermes rather than in `psionic`
- what Probe should consume directly
- what Probe should not rebuild
- what the first issue sequence should be for a Rust Probe CLI that talks to a
  local Psionic-served Qwen model

This document is intentionally Probe-facing.

It is about the integration seam and the backlog shape, not a historical
victory lap.

## Audit Basis

Primary `psionic` docs reviewed:

- `README.md`
- `docs/hermes/README.md`
- `docs/HERMES_QWEN35_COMPATIBILITY.md`
- `docs/HERMES_QWEN35_PARALLEL_ATTRIBUTION.md`
- `docs/HERMES_QWEN35_REUSE_BENCHMARK.md`
- `docs/HERMES_BACKEND_BENCHMARK.md`
- `docs/NON_GPT_OSS_QWEN35_PILOT.md`

Primary `psionic` code surfaces reviewed:

- `crates/psionic-serve/src/bin/psionic-openai-server.rs`
- `crates/psionic-serve/src/openai_http.rs`
- `crates/psionic-models/src/lib.rs`

Related planning context reviewed:

- `../README.md`
- `../AGENTS.md`

## Bottom Line

The important prior work is real.

`psionic` already proved that a local Qwen3.5 model can back a tool-using agent
over the normal OpenAI-compatible `chat.completions` path.

What `psionic` did **not** build was the controller product.

The split is:

- `psionic`
  - model serving, prompt rendering, OpenAI-compatible HTTP surface, tool-call
    contract handling, replay rules, and runtime receipts
- Hermes
  - the agent/controller loop that decides what to ask, when to call tools,
    how to continue the conversation, and how to present the interaction to a
    user

That boundary is exactly what Probe should exploit.

Probe should not reimplement Qwen serving, prompt templates, or backend
artifacts before it has a working controller.

Probe should consume `psionic-openai-server` first.

## What `psionic` Already Proved

## 1. Psionic is a real OpenAI-compatible backend for this lane

`psionic` exposes a real local OpenAI-compatible server:

- binary:
  `crates/psionic-serve/src/bin/psionic-openai-server.rs`
- route:
  `POST /v1/chat/completions`

The server takes one or more GGUF model paths and exposes them behind a local
HTTP endpoint.

For Probe, this matters because the first consumer path is already obvious:

- start `psionic-openai-server`
- point Probe at `http://127.0.0.1:<port>/v1`
- use the served GGUF basename as the model id

That is a clean controller/backend seam.

## 2. The Qwen3.5 family is not a vague future target

`psionic-models` already carries explicit Qwen3.5 support:

- tokenizer family:
  `qwen35`
- prompt-template family:
  `qwen35`
- decoder family:
  `Qwen35`
- multimodal projection metadata:
  `Qwen35MultimodalProjectionConfig`

This matters because Probe does not need to invent a prompt family abstraction
before it can talk to `psionic`.

For the first backend-consumption milestone, Probe should treat the model as a
remote capability exposed by the Psionic server, not as a local prompt-format
implementation problem.

## 3. Hermes-on-Psionic compatibility is already green on the retained matrix

The canonical retained compatibility doc says the local consumer-GPU lane is
green at `6/6` on `qwen3.5`.

Proven retained cases:

- required tool call
- plain-text no-tool answer
- multi-turn tool loop
- same-turn two-tool assistant response
- truthful refusal after invalid tool result
- streamed tool-call turn

That means the backend is already good enough for the first Probe controller
lane.

Probe does not need to prove raw backend viability from scratch.

## 4. The strict parallel-tool blocker was fixed in Psionic, not hand-waved away

The prior local blocker was the strict same-turn two-tool row.

The retained attribution docs make the fix explicit:

- required tool batches now prefer a plain JSON-schema batch
- required parallel turns now carry a concrete
  `minimum_required_tool_calls` floor
- that floor is inferred from declared tool names in the incoming request when
  the request is both `required` and parallel-capable
- the prompt contract then tells the model to emit at least that many tools in
  the required batch

The important Probe implication is direct:

- the tool lane that Probe targets should stay aligned with Psionic's current
  tool contract
- Probe should not invent a materially different initial tool envelope unless
  it is prepared to also re-solve the same backend behavior and replay issues

## 5. Tool-result replay already has concrete retention rules

`psionic-serve` includes tests and logic for replaying tool results into later
model turns.

One concrete example in `openai_http.rs` verifies that a tool result can infer
the tool name from a prior tool call when the tool message itself only carries
the call id.

This matters because the first Probe tool loop should rely on the same replay
shape that `psionic` already proved rather than ad hoc controller-side prompt
glue.

## 6. Prefix-cache reuse already matters for repeated tool loops

The retained reuse benchmark is not a small detail.

It shows material warm-path improvements on repeated Hermes-equivalent turns:

- `required_tool_turn`
  - warm wallclock improvement about `66.20%`
- `tool_result_continuation`
  - warm wallclock improvement about `75.80%`

The important product implication is:

- Probe should preserve repeated prompt prefixes where possible
- Probe should expose a prefix-cache policy, but default to the working backend
  path instead of fighting it

This is one reason to keep the first Probe backend path thin and honest.

## 7. Benchmark truth is mixed, not marketing-clean

The same-host benchmark evidence matters because it shows the honest current
boundary:

- Psionic passes the retained Hermes cases
- Ollama is still faster on the earlier four-case baseline rows
- Psionic wins the serialized two-city workflow that matters for the full local
  tool-backed path
- `llama.cpp` is not a clean comparator on the current host for this exact
  `qwen35` artifact contract

That leads to a useful product rule:

- Probe v0 should optimize for correctness and integration clarity first
- Probe should not assume "Psionic beats everything on all benchmarks" as a
  design premise

The backend choice is justified because it is owned, integrated, and already
proved on the target lane, not because it already wins every local benchmark.

## What Stayed In Hermes Instead Of Psionic

The prior work did **not** make `psionic` into a coding agent.

Hermes still owned:

- agent loop policy
- iteration limits
- user-facing task framing
- controller-side conversation management
- tool invocation decisions
- final answer behavior
- the actual CLI and agent UX

This is the strongest Probe takeaway in the entire audit.

Probe should become the owned controller that consumes `psionic`.

Probe should not try to turn `psionic` into the controller repo.

## What Probe Should Consume First

## 1. A local Psionic server process

The first working Probe backend lane should assume:

- `psionic-openai-server` is the backend process
- the backend is reachable over local HTTP
- Probe treats it as an OpenAI-compatible chat backend

This gives Probe a fast path to real end-to-end behavior without waiting on
native local inference code inside the Probe repo.

## 2. One canonical model lane

The first honest model lane should be a single retained Qwen3.5 artifact path.

Recommended first target:

- `qwen3.5-2b-q8_0-registry.gguf`

Why this is the right first target:

- it is the canonical retained compatibility row
- it already passed the full `6/6` matrix
- it is small enough for local iteration on one reachable consumer GPU
- it keeps the first Probe backend contract aligned with the strongest retained
  evidence

## 3. Plain `chat.completions` first

Probe should start by consuming the normal `chat.completions` path.

It should not begin with:

- Responses API assumptions
- structured outputs as a hard dependency
- multimodal input requirements
- provider-agnostic backend abstractions that erase the working Psionic seam

There is already enough proven surface in the current retained lane:

- chat turns
- tool calls
- replay
- streaming
- repeated prefix reuse

That is enough for a first controller.

## 4. Tool calling aligned to the retained Psionic contract

Probe should shape its first tool lane around the contract that `psionic`
already proved:

- declared tools
- `tool_choice`
- `parallel_tool_calls`
- plain JSON-style tool-call batches
- replay of tool results through later turns

The first Probe tool loop should avoid "creative" controller-side envelopes.

The point of this milestone is to consume a backend that already works.

## 5. No structured-output dependency in v0

The current `qwen35` text runtime in `psionic` still has an explicit boundary
around structured outputs.

That means Probe should not make structured-output support a blocker for the
first CLI milestone.

The controller can ship a useful first lane without it.

Probe v0 should prefer:

- plain assistant text
- tool-call batches
- explicit transcript items

over schema-heavy output contracts.

## What Probe Should Not Rebuild Yet

Probe should not start by rebuilding:

- local Qwen prompt rendering
- tokenizer behavior
- GGUF family parsing
- decoder-family classification
- serving transport
- CUDA inference
- multimodal projection
- backend-level parallel-tool fixes

Those are already `psionic` concerns.

The first Probe repo value is the controller, not duplicate substrate.

## First Probe CLI Milestone

The first real milestone should be:

- a Rust Probe CLI that can talk to a local `psionic-openai-server`
- against the retained `qwen3.5-2b-q8_0-registry.gguf` lane
- with a durable local session transcript
- with plain text turns first
- then with a bounded tool loop compatible with the current Psionic contract

This is a much tighter and more honest target than "build the full coding agent
all at once."

## Recommended Issue Sequence

Current GitHub issue map in `OpenAgentsInc/probe`:

- `#1`
  - Bootstrap Probe Rust workspace and canonical docs
- `#2`
  - Define the Probe session, turn, item, and transcript model
- `#3`
  - Add an OpenAI-compatible provider client crate for local backends
- `#4`
  - Add a first-class Psionic Qwen3.5 backend profile
- `#5`
  - Ship probe exec for plain-text turns against psionic-openai-server
- `#6`
  - Ship an interactive Probe CLI session loop with local resume
- `#7`
  - Implement the first Probe tool loop aligned with the current Psionic
    contract
- `#8`
  - Add Probe acceptance harnesses against a local Psionic Qwen lane
- `#9`
  - Add local launcher and attach mode for psionic-openai-server
- `#10`
  - Add controller-side observability for latency, throughput, and
    prefix-cache behavior

## 1. Bootstrap the Rust workspace and canonical docs

Goal:

- create the real Rust workspace shape for Probe
- add a docs baseline for runtime, protocol, and backend consumption

Why first:

- without this, later controller and backend work has no honest home

## 2. Define the Probe session, turn, item, and transcript model

Goal:

- define the durable controller-side runtime objects
- choose append-only transcript truth plus lightweight indexed metadata

Why second:

- the controller needs stable local truth before adding a backend adapter

## 3. Add an OpenAI-compatible provider client crate

Goal:

- implement a typed backend client for local OpenAI-compatible endpoints
- keep the first provider lane deliberately thin and synchronous enough to
  debug easily

Why third:

- this is the seam that lets Probe consume `psionic-openai-server`

## 4. Add a Psionic backend profile for Qwen3.5

Goal:

- add a first-class local backend config for:
  - base URL
  - model id
  - API key placeholder
  - prefix-cache behavior
  - timeout policy

Why fourth:

- the first supported backend should be explicit and tested, not hidden in
  environment-variable folklore

## 5. Ship `probe exec` plain-text turns against Psionic

Goal:

- allow one-shot CLI execution against a local Psionic-served Qwen model
- persist the turn transcript locally

Why fifth:

- this is the smallest end-to-end proof that Probe is a real controller

## 6. Ship an interactive Probe CLI session loop

Goal:

- add interactive chat-style use on top of the same session and provider stack
- support resume of local sessions

Why sixth:

- this turns the provider client into an actual runtime surface

## 7. Add the first Probe tool runtime aligned with Psionic's contract

Goal:

- implement declared tools, tool-choice policy, tool-result replay, and a
  bounded iteration loop

Why seventh:

- the backend already proved this lane under Hermes
- this is the first point where Probe begins to replace the old controller
  behavior rather than only sending text prompts

## 8. Add Probe acceptance harnesses against a local Psionic Qwen lane

Goal:

- codify retained controller-side acceptance cases for:
  - plain text no-tool turn
  - required single-tool turn
  - multi-turn tool continuation
  - same-turn two-tool batch

Why eighth:

- this gives Probe its own controller receipts while still relying on the
  already-proved Psionic backend

## 9. Add a local launcher and attach mode for `psionic-openai-server`

Goal:

- let Probe either:
  - attach to an already-running Psionic server
  - or spawn one as a supervised local child process

Why ninth:

- this removes manual operator friction without forcing Probe to own backend
  implementation details

## 10. Add performance and cache-behavior observability

Goal:

- surface request wallclock, token throughput, and cache behavior from the
  controller side

Why tenth:

- the reuse benchmark already proved warm-path value
- Probe should preserve visibility into whether it is actually getting that
  value in real sessions

## Design Rules For The First Lane

- Consume `psionic-openai-server` before attempting native local inference in
  Probe.
- Treat `qwen3.5-2b-q8_0-registry.gguf` as the first canonical backend target.
- Keep the first provider surface OpenAI-compatible and explicit.
- Keep structured outputs out of the critical path for Probe v0.
- Align the first tool loop with the current retained Psionic tool contract.
- Persist controller-side session truth in Probe from the beginning.
- Keep backend ownership in `psionic` and controller ownership in `probe`.

## Honest First Recommendation

If the goal is "a Rust Probe CLI next," the correct first move is not to port
Hermes wholesale and not to reimplement `psionic`.

The correct first move is:

1. build Probe as a Rust controller
2. talk to `psionic-openai-server`
3. lock the first lane to retained `qwen3.5-2b-q8_0-registry.gguf`
4. prove text turns
5. prove tool turns
6. only then widen the backend and runtime surface

That path reuses the strongest completed work instead of restarting the same
backend integration problem in a second repo.
