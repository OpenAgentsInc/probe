# Codex Third Inference Mode

Issue `#74` lands Codex as the third Probe inference mode alongside the
existing Qwen or Tailnet lane and the Apple FM lane.

This closes the loop on the earlier auth and transport work from
`54-openai-codex-subscription-auth.md` and
`55-openai-codex-subscription-backend.md`.

## Scope

Probe now treats Codex as a first-class backend lane instead of just a hidden
CLI profile.

That work spans:

- `crates/probe-core/src/harness.rs`
- `crates/probe-cli/src/main.rs`
- `crates/probe-tui/src/app.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-tui/src/widgets.rs`

The shipped behavior is:

- `probe tui` now cycles `Codex -> Qwen or Tailnet -> Apple FM`
- the Codex lane uses the canonical
  `openai-codex-subscription` backend profile
- the Codex backend overlay shows hosted-backend contract text plus local auth
  state from `PROBE_HOME/auth/openai-codex.json`
- tool-enabled Codex turns default to a Codex-specific Probe harness profile
  instead of the generic `coding_bootstrap_default`

## Prompt Contract

Codex now has backend-aware prompt resolution in `probe-core`.

### Tool-enabled Codex turns

If the caller enables:

- `--tool-set coding_bootstrap`
- and does not explicitly pass `--harness-profile`

Probe now resolves:

- harness profile: `coding_bootstrap_codex@v1`

That prompt keeps the same Probe-owned coding-tool contract as the default
coding bootstrap lane, but it is tuned for concise, action-oriented Codex
behavior.

### Plain Codex turns

If the caller does not enable a tool set, Probe now injects a small
Codex-specific plain system prompt that states:

- current working directory
- operating system
- expectation to inspect repo truth before making claims
- expectation to keep replies terse and verified

Qwen and Apple FM keep their prior plain-turn behavior.

### Operator addenda

The existing operator-system addendum path still works.

If the caller supplies `--system`, Probe appends it to the resolved Codex
harness prompt or to the Codex plain system prompt.

## TUI Behavior

The backend selector is now a three-lane selector:

- `Codex`
- `Qwen` or `Tailnet`
- `Apple FM`

If the OpenAI-compatible Qwen lane is pointed at a remote attach target, the
middle label remains `Tailnet` as before. Codex keeps the explicit `Codex`
label on the left.

Overlay behavior now differs by backend kind:

- Qwen or Tailnet shows the prepared attach target and remote-inference
  operator contract
- Codex shows the hosted ChatGPT Codex contract and the local subscription
  auth state
- Apple FM keeps the existing setup or availability overlay

Apple FM startup checks still only auto-run when Apple FM is the active lane.
Launching the TUI in Codex or Qwen does not fabricate Apple FM setup work.

## Reproduction

Authenticate once:

```bash
cargo run -p probe-cli -- codex login --method browser
```

Launch the TUI:

```bash
cargo run -p probe-cli -- tui
```

Then verify:

1. press `Tab` until the `Codex` lane is selected
2. press `Ctrl+S` to open the backend overlay
3. confirm the overlay shows:
   - `backend_kind: openai_codex_subscription`
   - `base_url: https://chatgpt.com/backend-api/codex`
   - `reasoning_level: backend_default`
   - `OpenAI Subscription Auth`
4. dismiss the overlay and submit a prompt
5. confirm the turn executes against Codex and persists a normal Probe session

CLI reproduction for the same lane:

```bash
cargo run -p probe-cli -- chat --profile openai-codex-subscription
```

Tool-enabled Codex reproduction:

```bash
cargo run -p probe-cli -- exec \
  --profile openai-codex-subscription \
  --tool-set coding_bootstrap \
  "Read README.md and summarize what Probe owns."
```

Expected behavior:

- the request goes through the hosted Codex subscription backend
- Probe uses `coding_bootstrap_codex@v1` automatically
- local tools, approvals, transcript storage, and UI stay Probe-owned

## Validation

Focused validation for this issue:

```bash
cargo test -p probe-core --lib
INSTA_UPDATE=always cargo test -p probe-tui
INSTA_UPDATE=always cargo test -p probe-cli
```
