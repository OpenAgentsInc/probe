# OpenAI Codex Subscription Auth

Issue `#72` adds Probe-owned subscription auth for OpenAI ChatGPT Plus/Pro or
Codex access without requiring `PROBE_OPENAI_API_KEY`.

This issue only lands the auth substrate and operator surfaces. The dedicated
Codex transport and third inference lane still live in `#73` and `#74`.

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

Stored fields:

- `refresh`
- `access`
- `expires`
- optional `account_id`

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

## Status And Logout

Inspect the current auth state:

```bash
cargo run -p probe-cli -- codex status
```

Clear the stored state:

```bash
cargo run -p probe-cli -- codex logout
```

## TUI Surface

`probe tui` does not run the interactive login flow itself yet, but the backend
overlay now shows:

- whether subscription auth is connected or disconnected
- the exact auth file path under `PROBE_HOME`
- whether the stored token is expired
- the saved `account_id` when present
- the CLI command to run next for login, status, or logout

That keeps the TUI honest about the local auth state before the dedicated
Codex inference lane lands.

## Reproduction Checklist

1. Start from a clean `PROBE_HOME` or remove `PROBE_HOME/auth/openai-codex.json`.
2. Run `cargo run -p probe-cli -- codex status` and confirm `authenticated=false`.
3. Run either browser or headless login.
4. Re-run `cargo run -p probe-cli -- codex status` and confirm:
   - `authenticated=true`
   - `expires_ms` is populated
   - `account_id` is populated when the subscription exposes one
5. Run `cargo run -p probe-cli -- codex logout`.
6. Re-run `cargo run -p probe-cli -- codex status` and confirm `authenticated=false`.

## Validation

Focused validation commands for this issue:

```bash
cargo test -p probe-openai-auth
cargo test -p probe-cli --test cli_regressions
cargo test -p probe-tui
cargo test -p probe-core --lib
```
