![Protoss Probe](assets/images/protossprobe.jpg)

# Probe

Probe is a coding agent.

It ships session persistence, tool execution, approvals, backend attachment,
and CLI/TUI surfaces for local coding work.

Current shipped surface:

- `probe exec` for one-shot turns
- `probe chat` for daemon-backed interactive sessions plus resume
- `probe codex login|status|logout` for multi-account ChatGPT/Codex
  subscription auth with live limit-aware routing and optional API-key
  fallback
- `probe tui` / `cargo probe` for the local terminal UI plus explicit
  detached-session reattach with `--resume`
- a Codex-first TUI shell with backend autodetection, no interactive backend
  selector strip on the hot path, Codex-style bullet-led transcript rows,
  tree-prefixed tool details, dot-separated footer metadata, syntax-highlighted
  fenced code blocks, and background GitHub issue selection metadata when `gh`
  is available
- `probe-server` for the first typed local stdio supervision contract
- `probe-daemon` for the first long-lived local Unix-socket supervision path
- `probe daemon run|stop` plus `probe ps|attach|logs|stop` for local detached
  session supervision
- a shared `probe-client` layer underneath `exec`, `chat`, and the TUI so
  first-party surfaces now speak one server contract
- `coding_bootstrap` tools, approvals, and harness profiles
- append-only local transcripts under `PROBE_HOME` or `~/.probe`
- bounded oracle and long-context escalation lanes
- local acceptance/eval and module-optimization tooling
- private Forge worker-attachment support in `probe-core::forge_worker` for
  persisted worker-session state, attach, heartbeat, and revocation handling
- Forge-owned RLM assignment execution in `probe-core::forge_rlm` plus
  `probe forge rlm execute|proof-openagents-4368` for large-corpus issue-thread
  analysis with chunk manifests, trace artifacts, and grounded outputs

## Backends

Probe currently ships four backend profiles across three backend families:

- `psionic-qwen35-2b-q8-registry`
  - base URL: `http://127.0.0.1:8080/v1`
  - model: `qwen3.5-2b-q8_0-registry.gguf`
- `psionic-inference-mesh`
  - attach target: Psionic OpenAI-compatible server plus
    `/psionic/management/status`
  - default base URL: `http://127.0.0.1:8080/v1`
  - selected model: resolved from live mesh management state
  - stored metadata: routed model inventory, local mesh role or posture, and
    proxied fallback truth
  - optional adjunct: session-scoped coordination reads or posts through
    `/psionic/management/coordination/*` without mutating Probe transcript or
    approval truth
  - optional adjunct: typed mesh plugin-offer publish or list calls so one
    Probe node can advertise its local tool bundle to other attached operators
    without pretending Psionic owns those tools
- `openai-codex-subscription`
  - base URL: `https://chatgpt.com/backend-api/codex`
  - request endpoint: `https://chatgpt.com/backend-api/codex/responses`
  - model: `gpt-5.4`
  - reasoning level: `backend_default`
  - auth source: versioned multi-account state at `PROBE_HOME/auth/openai-codex.json`
  - optional fallback env: `PROBE_OPENAI_API_KEY`
  - workspace secret autoload: `.secrets/probe-openai.env` when Probe starts
    inside that workspace tree
- `psionic-apple-fm-bridge`
  - default base URL: `http://127.0.0.1:11435`
  - model: `apple-foundation-model`
  - override order: `PROBE_APPLE_FM_BASE_URL`, then `OPENAGENTS_APPLE_FM_BASE_URL`

Apple FM is attach-only. Probe checks `GET /health` before use and stays honest
about unavailable or non-admitted machines.
Codex is also attach-only, but its attach target is the hosted ChatGPT Codex
endpoint rather than a local Psionic server.
Probe now supports multiple saved Codex subscription accounts in one auth file,
refreshes them independently, polls the hosted usage endpoint to estimate
headroom, prefers the account with the most remaining capacity, and can fall
back to `PROBE_OPENAI_API_KEY` when the saved subscription accounts are
missing, unusable, or rate-limited. If Probe starts inside a workspace tree
that contains `.secrets/probe-openai.env`, it autoloads that key into the CLI
process before the TUI, `exec`, `chat`, or `codex status` paths run.
The mesh profile is attach-only as well. Probe discovers live routed inventory
from `GET /psionic/management/status`, picks the effective model from that
inventory, prints the mesh role or fallback posture in operator output, and
stores the same typed snapshot in session metadata without pretending it owns
mesh startup or warmup. Probe can also query or post the optional mesh
coordination adjunct for a session through the runtime or `probe-client`
surface, and can publish or inspect typed mesh plugin offers for local Probe
tool bundles. That data remains outside the append-only transcript and outside
pending-approval invariants, and the tools themselves still execute inside the
local Probe runtime that published the offer.
Tool-enabled Codex turns default to the Probe-owned
`coding_bootstrap_codex@v1` prompt contract, while plain Codex turns use a
small Codex-specific system prompt instead of the generic local-Qwen path.

## Quick Start

Install the published macOS arm64 CLI:

```bash
npm i -g @openagentsinc/probe
probe
probe exec --profile openai-codex-subscription "hello"
```

If you want bare `probe` to auto-use an OpenAI API key inside this workspace,
create:

```bash
mkdir -p .secrets
chmod 700 .secrets
cat > .secrets/probe-openai.env <<'EOF'
PROBE_OPENAI_API_KEY=sk-...
EOF
chmod 600 .secrets/probe-openai.env
```

Probe walks upward from the current working directory looking for that file. If
you launch Probe outside that workspace tree, export `PROBE_OPENAI_API_KEY`
normally instead.

The npm install path is currently mac-first for Apple silicon. Packaging and
release details live in `docs/82-mac-first-npm-global-install-packaging.md`
and `docs/83-mac-first-npm-release-staging.md`.

Bare `probe` now opens the TUI by default. The default TUI path is Codex-first
and no longer exposes the old backend selector strip in the primary shell.
Work-shaped submitted prompts are treated as priority strings for background
GitHub issue selection across discoverable local sibling repos. Casual chat or
identity questions no longer hit that path. When `gh` finds a match, the
footer and transcript show the selected issue metadata. When no issue matches,
the transcript records that cleanly instead of pretending a selection exists.
Transcript rows, tool output, and the footer/composer metadata now use the same
semantic color direction as Codex: cyan links, paths, issue refs, and inline
code; magenta slash commands and reasoning metadata; green quote/status accents;
syntax-highlighted fenced code blocks via a Probe-owned Rust renderer; and
bullet-led `Calling` / `Called` tool history rows with dim `  └ ` detail lines
instead of bracketed log labels.
Committed runtime/backend failures now render as structured multi-line status
rows with typed metadata like `session`, `status`, `plan`, and `reset_in`
instead of dumping one raw backend error blob into the transcript.

Run the TUI:

```bash
cargo probe
```

Run a one-shot turn:

```bash
cargo run -p probe-cli -- exec "Explain what this repository does."
```

Start an interactive session:

```bash
cargo run -p probe-cli -- chat
```

Start an interactive Codex-backed session:

```bash
cargo run -p probe-cli -- chat --profile openai-codex-subscription
```

Start an interactive Psionic mesh-backed session:

```bash
cargo run -p probe-cli -- chat \
  --profile psionic-inference-mesh \
  --server-host 100.88.7.9 \
  --server-port 8080
```

Publish the local `coding_bootstrap` tool bundle for a mesh-backed session:

```bash
cargo run -p probe-cli -- mesh plugins publish <session-id>
```

List published tool-bundle offers for a mesh-backed session:

```bash
cargo run -p probe-cli -- mesh plugins list <session-id>
```

Run the local stdio server contract directly:

```bash
cargo run -p probe-server -- --probe-home ~/.probe
```

Run the long-lived local daemon:

```bash
cargo run -p probe-daemon -- run --probe-home ~/.probe
```

Tune detached watchdog budgets when needed:

```bash
cargo run -p probe-cli -- daemon run \
  --probe-home ~/.probe \
  --watchdog-stall-ms 30000 \
  --watchdog-timeout-ms 300000
```

Inspect detached sessions:

```bash
cargo run -p probe-cli -- ps --probe-home ~/.probe
```

Inspect one detached session:

```bash
cargo run -p probe-cli -- attach <session-id> --probe-home ~/.probe
```

Tail detached session logs:

```bash
cargo run -p probe-cli -- logs <session-id> --probe-home ~/.probe --follow
```

Stop detached work for one session:

```bash
cargo run -p probe-cli -- stop <session-id> --probe-home ~/.probe
```

Stop the daemon after detached work is drained:

```bash
cargo run -p probe-cli -- daemon stop --probe-home ~/.probe
```

Resume a session:

```bash
cargo run -p probe-cli -- chat --resume <session-id>
```

Reattach the TUI to a detached session:

```bash
cargo run -p probe-cli -- tui --resume <session-id>
```

Run a tool-enabled turn:

```bash
cargo run -p probe-cli -- exec \
  --tool-set coding_bootstrap \
  --harness-profile coding_bootstrap_default \
  --tool-choice auto \
  "Read README.md and summarize what this repository does."
```

## Auth With ChatGPT

Probe can use your ChatGPT subscription for the hosted Codex lane. This does
not require `PROBE_OPENAI_API_KEY` when subscription auth is healthy.

Prerequisite:

- sign in to a ChatGPT account with the Codex-capable subscription you want to use

Recommended browser flow:

```bash
# start local browser auth
cargo run -p probe-cli -- codex login --method browser
```

Probe will print the authorize URL, open the browser when possible, wait for
the localhost callback, and then persist the resulting auth state at
`PROBE_HOME/auth/openai-codex.json`.

Recommended worker or headless flow:

```bash
# device-code auth for a worker machine or SSH-only host
cargo run -p probe-cli -- codex login --method headless
```

If you are on a headless machine or do not want Probe to open the browser, you
can also:

```bash
# browser auth, but copy the URL manually
cargo run -p probe-cli -- codex login --method browser --no-open-browser
```

Verify or clear the saved ChatGPT auth:

```bash
# inspect the saved auth record
cargo run -p probe-cli -- codex status

# delete the saved auth record
cargo run -p probe-cli -- codex logout
```

Probe persists this state at `PROBE_HOME/auth/openai-codex.json` with private
file permissions. The current TUI backend overlay also shows whether that auth
state exists and whether it is expired. `probe codex status` now also prints a
worker-oriented hint for the headless device flow.

## OpenAI API Key

Probe can also drive the default Codex-first lane with a plain OpenAI API key.

Preferred workspace-local setup:

```bash
mkdir -p .secrets
chmod 700 .secrets
cat > .secrets/probe-openai.env <<'EOF'
PROBE_OPENAI_API_KEY=sk-...
EOF
chmod 600 .secrets/probe-openai.env
```

Then run Probe from anywhere inside that workspace tree:

```bash
probe
```

Probe autoloads `.secrets/probe-openai.env`, exposes the key as
`PROBE_OPENAI_API_KEY` inside the CLI process, and lets the default
`openai-codex-subscription` lane route to the public Responses API when
subscription auth is missing, expired, or rate-limited.

If you are not running inside that workspace tree, use a normal shell export:

```bash
export PROBE_OPENAI_API_KEY=sk-...
probe
```

Operator surfaces now make that route visible:

- `probe codex status` prints `api_key_source=...` and `selected_route=...`
- the TUI footer shows `api key` when the active Codex lane is currently using
  the key-backed route
- the backend overlay shows `selected_route`, `api_key_fallback`, and the key
  source summary

## Forge Worker Auth

Probe now has a private Forge worker-auth support layer in
`probe-core::forge_worker`.

That layer is intentionally runtime-scoped:

- Probe stores the persisted Forge worker session under
  `PROBE_HOME/auth/forge-worker.json`
- Probe can consume Forge bootstrap credentials and exchange them for a
  short-lived worker session token
- Probe can emit worker heartbeats through the Forge worker-auth contract
- Probe clears the local worker session automatically if Forge returns
  unauthorized, so revocation or expiry does not leave stale runtime auth on
  disk

Probe also has a private Forge-assigned run execution layer in
`probe-core::forge_run_worker`.

That runtime path is still deliberately subordinate to Forge:

- Probe can inspect its current assigned Forge Run
- Probe can claim the next Forge-assigned Run when the worker is idle
- Probe can execute one assigned coding turn through `ProbeRuntime`
- Probe emits Forge runtime lifecycle events such as `run.started`,
  `run.progress`, `run.ready_for_verification`, and `run.failed`
- Probe reports the linked runtime session id back to Forge so Forge can bind
  Run lifecycle state to runtime truth
- Probe includes transcript refs, retained summary artifact refs, and recovery
  context in `run.ready_for_verification` summaries so Forge can assemble
  Evidence Bundles without guessing
- Probe emits an explicit resume-style `run.progress` event when a restarted
  worker finds an already-active Forge Run, so Forge can track recovery
  honestly without making Probe the lifecycle authority

Probe still does not become the authority for Work Orders, Runs, Leases,
Evidence, Verification, or Delivery.

Probe now also has a private Forge-owned RLM execution lane in
`probe-core::forge_rlm`.

That lane keeps the split explicit:

- Forge owns the canonical strategy family, policy bundle, runtime-assignment
  contract, and issue-thread evaluator crates
- Probe consumes the typed Forge assignment and execution plan
- Probe materializes large corpora as local inputs, builds a chunk manifest,
  emits explicit execution events, and writes replayable artifacts
- Probe returns structured outputs, trace refs, and artifact refs without
  pretending the runtime is now the policy authority

The first operator proof is the full live GitHub issue thread for
`OpenAgentsInc/openagents#4368`.

Probe now also has a typed issue-thread strategy route above that execution
lane:

- `issue_thread_direct_v1`
  - direct single-shot issue-thread analysis
- `paper_rlm_issue_thread_v1`
  - paper-style recursive issue-thread analysis
- `heuristic_rlm_trigger_v1`
  - typed trigger receipt that chooses between them

Dry-run the route decision without executing a model:

```bash
cargo run -p probe-cli -- forge rlm analyze-issue-thread \
  --issue-url https://github.com/OpenAgentsInc/openagents/issues/4368 \
  --query "What is the current blocker?" \
  --strategy auto \
  --dry-run
```

Force execution under the paper RLM lane:

```bash
cargo run -p probe-cli -- forge rlm analyze-issue-thread \
  --issue-url https://github.com/OpenAgentsInc/openagents/issues/4368 \
  --query "What is the current blocker?" \
  --strategy rlm \
  --output-dir var/forge-rlm
```

Run an arbitrary Forge RLM plan:

```bash
cargo run -p probe-cli -- forge rlm execute \
  --plan /path/to/forge-rlm-plan.json \
  --output-dir var/forge-rlm
```

Run the checked-in live `#4368` proof:

```bash
GH_TOKEN=$(gh auth token) cargo run -p probe-cli -- \
  forge rlm proof-openagents-4368 \
  --output-dir var/forge-rlm
```

That command writes ignored artifacts under `var/forge-rlm/<label>-<timestamp>/`:

- `assignment.json`
- `corpus.json`
- `corpus.md`
- `chunk_manifest.json`
- `report.json`
- `trace.json`
- `events.json`
- `runtime_result.json`
- `brief.md`

See
[`docs/84-forge-rlm-assignment-execution.md`](docs/84-forge-rlm-assignment-execution.md)
for the execution envelope, chunking rules, and live-proof validation path, and
[`docs/85-issue-thread-strategy-routing.md`](docs/85-issue-thread-strategy-routing.md)
for the direct-versus-RLM route receipt and TUI override surface.

Probe now also has a retained paper-RLM comparison report in
`probe-core::issue_thread_eval`.

Run the synthetic acceptance report:

```bash
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo test -p probe-core \
  comparison_report_requires_externalization_and_recursive_subcalls \
  -- --nocapture
```

Run the gated live `OpenAgentsInc/openagents#4368` comparison:

```bash
GH_TOKEN=$(gh auth token) PROBE_OPENAI_API_KEY=... \
  CARGO_NET_GIT_FETCH_WITH_CLI=true cargo test -p probe-core \
  live_openagents_4368_comparison_reads_the_full_current_thread \
  -- --ignored --nocapture
```

See [`docs/86-paper-rlm-evals.md`](docs/86-paper-rlm-evals.md) for the report
shape and the externalization checks.

The first-party worker CLI now sits on top of that runtime layer:

```bash
# inspect local Forge worker attachment state
cargo run -p probe-cli -- forge status --probe-home ~/.probe

# attach a Probe worker to Forge with a bootstrap credential
cargo run -p probe-cli -- forge attach \
  --probe-home ~/.probe \
  --forge-base-url http://127.0.0.1:8080 \
  --worker-id forge-worker-123 \
  --bootstrap-token <forge-bootstrap-token>

# inspect or claim assigned Forge work
cargo run -p probe-cli -- forge current-run --probe-home ~/.probe
cargo run -p probe-cli -- forge claim-next --probe-home ~/.probe

# execute one assigned Forge Run through the real Probe runtime
cargo run -p probe-cli -- forge run-once \
  --probe-home ~/.probe \
  --cwd /path/to/worktree

# stay attached as a worker and keep polling for new assignments
cargo run -p probe-cli -- forge run-loop --probe-home ~/.probe
```

Those commands keep the boundary explicit:

- Forge stays authoritative for work lifecycle state
- Probe stores only the local worker session and runtime artifacts
- `probe forge run-once` and `probe forge run-loop` are worker commands, not a
  replacement control plane

For the first boring private Linux deploy lane, use
`scripts/deploy/forge-worker/`:

- `01-provision-baseline.sh`
- `02-configure-and-start.sh`
- `03-run-headless-codex-login.sh`
- `04-open-logs.sh`
- `05-refresh-attachment.sh`
- `06-restart-service.sh`
- `99-local-smoke.sh`

That lane is documented in
[`docs/81-codex-backed-forge-worker-deploy-lane.md`](docs/81-codex-backed-forge-worker-deploy-lane.md)
and keeps the auth split explicit:

- Codex subscription auth lives at `PROBE_HOME/auth/openai-codex.json`
- Forge worker-session auth lives at `PROBE_HOME/auth/forge-worker.json`
- `PROBE_OPENAI_API_KEY` is optional for the default Codex lane and is only
  needed when Probe should use the Responses API fallback path

After auth succeeds, use the ChatGPT-backed Codex profile directly:

```bash
# interactive Codex chat
cargo run -p probe-cli -- chat --profile openai-codex-subscription

# one-shot Codex turn
cargo run -p probe-cli -- exec \
  --profile openai-codex-subscription \
  "Reply with the exact text: codex backend ready"
```

In the TUI, launch `cargo probe` or `probe`. The default path now prefers the
Codex-backed TUI automatically and drops you straight into the transcript
shell. When the active Codex route is running on `PROBE_OPENAI_API_KEY`, the
footer picks up an `api key` badge so the operator can see that the session is
not currently using subscription bearer auth.

The canonical Codex backend profile prefers
`https://chatgpt.com/backend-api/codex/responses` with subscription bearer
auth, but now falls through to the public Responses API when
`PROBE_OPENAI_API_KEY` is available and the saved subscription route is not
usable.

The OpenAI-compatible Psionic-backed profiles are different: they use the env
var named by the profile, currently `PROBE_OPENAI_API_KEY`. Probe now resolves
that env var explicitly, including the workspace-secret autoload path above,
and fails early if the profile requires it and the key is still missing or
empty.

## TUI

`cargo probe` is the current top-level Probe UI entrypoint. The current shell
uses a retained transcript widget with committed user, tool, and assistant
turns plus one explicit active-turn cell. `Chat` is the home surface and the
composer now submits through the real Probe session loop. `probe tui` now uses the
same prepared backend contract as `probe chat`: it resolves the chosen
backend, runs server readiness or attach preparation first, and then carries
the prepared host, port, model, backend kind, and attach mode into the UI.
The first submit creates a persisted Probe session; later submits continue that
same session. The active-turn cell is now driven by real session lifecycle
events, so the TUI can show model requests, tool request/start/completion,
refusal or pause, and the final assistant commit before the transcript delta
is rendered. Persisted tool activity renders as first-class transcript rows
such as `[tool call]`, `[tool result]`, and `[approval pending]` rather than
generic notes.

Probe now distinguishes backend streaming truth explicitly:
OpenAI-compatible backends stream assistant deltas, while Apple FM
streams full session snapshots rather than fake token deltas. The chat surface
now renders those streams honestly in place: one retained active cell grows
with streamed deltas or snapshot replacement until the authoritative
transcript rows land, streamed tool-call assembly stays visible before final
tool rows commit, and the input header carries the compact backend and stream
summary instead of a separate status box.

Setup, help, and approval flows live in a typed overlay stack above or in
place of the composer. The old setup surface is now a backend overlay:
Apple FM launches still foreground local Apple FM admission and setup truth,
while Qwen or Tailnet launches show the prepared attach target and operator
contract. The Codex lane instead shows the hosted backend contract plus local
ChatGPT subscription auth status from `PROBE_HOME/auth/openai-codex.json`.

The first supported remote-Qwen posture stays narrow and explicit:

- local Probe owns sessions, transcripts, tools, approvals, and UI
- remote Psionic serves inference only
- `127.0.0.1` attach is treated as local or SSH-forwarded
- `100.x.y.z` attach is treated as direct Tailnet attach

The default TUI path is now Codex-first and no longer exposes the old backend
selector strip or backend-cycling hotkeys in the primary shell. Qwen and Apple
FM code paths still exist for explicit backend/profile work, but they are not
part of the normal user-facing TUI hot path.

The default `cargo probe` chat lane now auto-approves local tools and keeps
tool transcript rows terse. Tool calls and results render as compact command,
path, or error summaries instead of debug-shaped JSON blobs.

Keys:

- `Enter`: submit the composer
- `Shift+Enter`: insert a newline
- `Up`, `Down`: recall draft history
- `Ctrl+O`: add an attachment placeholder to the draft
- `Ctrl+R`: rerun backend check when supported
- `Ctrl+S`: open backend overlay
- `Ctrl+A`: open approval overlay
- `Ctrl+T`: toggle operator notes vs live detail
- `F1`: help
- `Esc`: dismiss modal
- `Ctrl+C`: quit

The composer is active on the main transcript shell and disabled while overlays
own focus. The draft model tracks slash commands, typed mentions such as
`@skill:rust` or `@app:github`, attachment placeholders, submission history,
and multiline paste state. When a tool pauses for approval, Probe now persists
a real pending-approval record in `probe-core`, opens the approval overlay with
the live tool details, and resumes the paused turn after approve or reject
instead of leaving the operator in a dead-end pause state.

Exact zero-argument slash commands stay local to the shell instead of being
sent to the runtime:

- `/help`
- `/backend`
- `/approvals`
- `/reasoning`
- `/clear`

Anything else, including slash-prefixed prompts with extra text, still goes
through the normal Probe runtime turn path. Stream failures now render typed
recovery guidance in the transcript, such as Codex reauth or local-backend
restart advice, while keeping any partial assistant output visible.

Remote attach examples:

```bash
# direct Tailnet attach
cargo run -p probe-cli -- tui \
  --profile psionic-qwen35-2b-q8-registry \
  --server-host 100.88.7.9 \
  --server-port 8080

# SSH-forwarded localhost attach
cargo run -p probe-cli -- tui \
  --profile psionic-qwen35-2b-q8-registry \
  --server-host 127.0.0.1 \
  --server-port 8080
```

## Dev Helpers

Repo-local helpers:

```bash
./probe-dev fmt
./probe-dev check
./probe-dev test
./probe-dev integration
./probe-dev accept-live
```

Probe intentionally does not keep GitHub CI workflows. Run the precommit lane
locally before pushing:

```bash
./probe-dev pr-fast
```

Useful focused lanes:

- `./probe-dev pr-fast`
- `./probe-dev cli-regressions`
- `./probe-dev integration`
- `./probe-dev accept-live`
- `./probe-dev self-test`
- `./probe-dev accept-compare`
- `./probe-dev matrix-eval`
- `./probe-dev optimizer-eval <lane>`

## Docs

Start with the optimizer-system doc, the runtime ownership doc, and the
workspace map if you need to understand how Probe is split today and where the
offline optimizer fits. For deeper TUI, backend, and streaming design history,
use `docs/README.md`; the full contributor start-here list now lives in
`AGENTS.md`.
