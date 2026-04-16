# OpenAI Codex Subscription Auth

Issue `#72` adds Probe-owned subscription auth for OpenAI ChatGPT Plus/Pro or
Codex access without requiring `PROBE_OPENAI_API_KEY`.

The auth substrate now backs the shipped Codex backend transport from `#73`
and the third TUI inference lane from `#74`.

## Ownership

The auth substrate now lives in the new shared crate:

- `crates/probe-openai-auth/src/lib.rs`

Operator surfaces that use it:

- `crates/probe-cli/src/main.rs`
- `crates/probe-cli/tests/cli_regressions.rs`
- `crates/probe-tui/src/screens.rs`
- `crates/probe-tui/src/app.rs`

## Stored State

Probe stores subscription auth at:

- `PROBE_HOME/auth/openai-codex.json`

That file is now a Probe-owned versioned multi-account store rather than one
flat token record.

Per-account stored fields include:

- stable local `key`
- optional `label`
- `refresh`
- `access`
- `expires`
- optional `account_id`
- `added_at_ms`
- optional `last_selected_at_ms`
- optional cached rate-limit snapshot from `GET /backend-api/wham/usage`

The store also keeps the current `selected_account_key`.

Probe still reads the original single-record JSON shape for backward
compatibility and transparently lifts it into the multi-account view.

On Unix systems the file is written with `0600` permissions.

## Supported Flows

### Browser PKCE

Run:

```bash
cargo run -p probe-cli -- codex login --method browser
```

Probe will:

1. bind a localhost callback listener
2. build the OpenAI authorize URL with PKCE and `originator=probe`
3. print the authorize URL and redirect URI
4. try to open the browser unless `--no-open-browser` is set
5. wait for the callback, exchange the authorization code, and persist tokens

Use `--no-open-browser` if you want to copy the URL manually:

```bash
cargo run -p probe-cli -- codex login --method browser --no-open-browser
```

To add a readable local label for later status output:

```bash
cargo run -p probe-cli -- codex login --method browser --label work
```

### Headless Device Flow

Run:

```bash
cargo run -p probe-cli -- codex login --method headless
```

Probe will:

1. request a device auth code from OpenAI
2. print the verification URL and user code
3. poll for authorization readiness
4. exchange the returned authorization code
5. persist the resulting token state

This is the preferred operator flow for worker machines, SSH-only hosts, and
the first private Forge worker lane.

The checked-in worker deploy lane lives under
`scripts/deploy/forge-worker/` and wraps this exact command through
`03-run-headless-codex-login.sh`. The canonical stored path remains:

- `PROBE_HOME/auth/openai-codex.json`

You can also attach a label on this path:

```bash
cargo run -p probe-cli -- codex login --method headless --label personal
```

## Smart Selection And Fallback

When Probe executes against the `openai-codex-subscription` backend now, it:

1. refreshes each saved account independently when needed
2. refreshes cached usage snapshots from `GET /backend-api/wham/usage`
3. ranks non-expired accounts by remaining headroom
4. prefers the account with the lowest `used_percent`
5. rotates to the next account if the chosen account still returns `429`
6. optionally falls back to `PROBE_OPENAI_API_KEY` when the connected
   subscription accounts are missing, unusable, or rate-limited

That keeps one exhausted or stale account from blocking the others.

Probe CLI startup now also autoloads `PROBE_OPENAI_API_KEY` from a workspace
secret file named `.secrets/probe-openai.env` when the current working
directory is inside that workspace tree.

## Status And Logout

Inspect the current auth state:

```bash
cargo run -p probe-cli -- codex status
```

Status now prints:

- account count
- selected account key and label
- per-account expiry and cached rate-limit summary
- the selected execution route
- whether `PROBE_OPENAI_API_KEY` is currently available as an optional fallback

When auth is missing, Probe still prints both:

- a local browser-login hint
- a worker-oriented headless-login hint

Clear the stored state:

```bash
cargo run -p probe-cli -- codex logout
```

Remove one saved account without clearing the rest:

```bash
cargo run -p probe-cli -- codex logout --account acct_123
```

## TUI Surface

`probe tui` does not run the interactive login flow itself yet, but the backend
overlay now shows:

- whether subscription auth is connected or disconnected
- the exact auth file path under `PROBE_HOME`
- whether the stored token is expired
- the saved `account_id` when present
- the CLI command to run next for login, status, or logout

That keeps the TUI honest about the local auth state before the Codex lane
submits any real request.

## Reproduction Checklist

1. Start from a clean `PROBE_HOME` or remove `PROBE_HOME/auth/openai-codex.json`.
2. Run `cargo run -p probe-cli -- codex status` and confirm `authenticated=false`.
3. Run either browser or headless login.
4. Re-run `cargo run -p probe-cli -- codex status` and confirm:
   - `authenticated=true`
   - `account_count` increments
   - `expires_ms` is populated for the selected account
   - `account_id` is populated when the subscription exposes one
   - the saved path is `PROBE_HOME/auth/openai-codex.json`
5. Add a second account and confirm the status output lists both accounts.
6. Run `cargo run -p probe-cli -- codex logout --account <account_id>` and confirm only that one account is removed.
7. Run `cargo run -p probe-cli -- codex logout`.
8. Re-run `cargo run -p probe-cli -- codex status` and confirm `authenticated=false`.

## Validation

Focused validation commands for this issue:

```bash
cargo test -p probe-openai-auth
cargo test -p probe-cli --test cli_regressions
cargo test -p probe-tui
cargo test -p probe-core --lib
```

For the first private Forge worker deployment path, also validate:

```bash
bash scripts/deploy/forge-worker/99-local-smoke.sh
```
