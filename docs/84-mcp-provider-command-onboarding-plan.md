# MCP Provider-Command Onboarding Plan

This plan makes MCP setup in Probe feel closer to the workflows users already
know from Claude Code, Codex, Cursor, and provider docs.

The key product principle is simple:

- users should not need to understand Probe's internal storage model before
  they can add an MCP
- the default MCP path should start from the command providers already publish
- raw fields like transport and target should exist only as an advanced escape
  hatch

## UX Bar

Probe should feel familiar when a provider gives instructions like:

```bash
pnpm dlx shadcn@latest mcp init --client codex
```

The operator expectation is:

1. open `/mcp`
2. choose `Add MCP`
3. paste the provider command from the docs
4. see a clear next step or imported preview
5. save with confidence

Probe should not force the operator to reverse engineer "transport" and
"target" just to match a provider guide.

## Current Truth

Today Probe can:

- open a real `/mcp` manager
- add saved MCP entries
- enable or disable saved entries
- persist those entries in `PROBE_HOME/mcp/servers.json`

Today Probe cannot:

- execute external MCP servers during turns
- import provider setup commands automatically
- discover external MCP tool inventories
- show live MCP connection state or per-turn MCP receipts

## Phase 1: Onboarding Language And Entry Paths

Status: `[x]`

### Outcome

Probe exposes a standard-looking MCP add flow instead of dropping straight into
raw config fields.

### Scope

- [x] add an explicit add-method chooser after `Add MCP server`
- [x] make `Paste provider setup command` the default path
- [x] keep `Manual setup (advanced)` as the escape hatch
- [x] rename the manual fields into operator language:
  - `Display name`
  - `Connection type`
  - `Launch command or URL`
- [x] carry pasted provider commands forward as visible setup reference when
      the operator falls through to manual setup

### Exit Criteria

- [x] `/mcp` no longer relies on hidden keyboard knowledge for the add flow
- [x] the first add screen matches normal MCP mental models
- [x] the manual form no longer reads like Probe-internal schema

## Phase 2: Provider Command Capture And Saved Recipes

Status: `[ ]`

### Outcome

Probe can save provider-command-based MCP recipes as first-class entries, even
before generic runtime execution exists.

### Scope

- [ ] extend the saved MCP registry to distinguish:
  - manual launch entries
  - provider setup command recipes
- [ ] show recipe entries clearly in `/mcp`
- [ ] preserve provider command, inferred provider name, and intended client
- [ ] avoid pretending recipe entries are already executable

### Exit Criteria

- [ ] a user can paste a provider command and save it without translating it by
      hand first
- [ ] saved recipe entries are clearly distinguished from manual launch entries
- [ ] Probe never suggests recipe entries are live runtime integrations

## Phase 3: Import Adapters For Known Providers

Status: `[ ]`

### Outcome

Probe can interpret a small supported set of provider setup commands and
convert them into richer Probe-managed entries.

### Scope

- [ ] define an adapter seam for known providers
- [ ] start with one or two popular providers such as `shadcn`
- [ ] infer sensible defaults like display name and likely connection type
- [ ] show a preview before saving any imported result
- [ ] keep unknown commands on the recipe fallback path

### Exit Criteria

- [ ] supported provider commands feel close to "paste command, review, save"
- [ ] unsupported commands fail honestly and fall back safely

## Phase 4: Probe-Native Provider Target

Status: `[ ]`

### Outcome

Probe becomes a first-class documented MCP client target instead of only
borrowing Codex or Claude Code setup guides.

### Scope

- [ ] define Probe's preferred MCP client identity
- [ ] document the target for providers
- [ ] add copy in `/mcp` that prefers Probe-native instructions when available

### Exit Criteria

- [ ] Probe can point users at a stable provider-facing target such as
      `--client probe`
- [ ] providers have enough information to document Probe cleanly

## Phase 5: Runtime Wiring

Status: `[ ]`

### Outcome

Saved MCP entries become real runtime integrations with connection state, tool
inventory, and turn receipts.

### Scope

- [ ] mount enabled MCP entries into `probe-core` and `probe-server`
- [ ] expose live MCP connection state in the runtime protocol
- [ ] show tool inventories and connection failures in `/mcp`
- [ ] record MCP usage in receipts and runtime activity

### Exit Criteria

- [ ] enabled MCP entries can actually be used during turns
- [ ] `/mcp` shows configured versus connected truth
- [ ] final task receipts include MCP usage when it occurs

## Validation Checklist

- [ ] add an MCP from the provider-command path without seeing raw transport
      language first
- [ ] add an MCP from the manual path when you already know the launch command
- [ ] save a provider recipe and verify it survives restart
- [ ] verify unsupported provider commands fail with clear next steps
- [ ] once runtime support lands, verify connected state and tool counts are
      visible from `/mcp`
