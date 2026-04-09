# MCP Runtime Usability Plan

This plan closes the biggest current MCP gap in Probe:

- Probe can now run connected manual `stdio` MCP servers end to end
- but the default operator flow still lands on saved provider recipes that are
  not actually usable during turns

That means Probe is telling the truth, but it is not yet delivering the
operator promise:

"If I add an MCP from provider docs, I can understand what happened and get it
working without reverse engineering Probe internals."

## UX Standard

Probe should make MCP feel obvious in the same way Claude Code, Codex, and
other best-in-class coding tools make model or workspace state obvious.

The operator should be able to:

1. add an MCP from provider docs
2. understand whether it is only saved, runnable, connected, or failing
3. fix the problem from inside Probe
4. confirm the MCP is actually available in the current session
5. see when the model used it

Probe should not require the operator to infer the meaning of:

- provider recipe
- runtime session attachment
- transport support
- why an MCP is enabled but still unusable

## Current Gaps

### Highest Priority

1. Provider recipes are not runnable
   - the default provider-command path creates entries that look important but
     are not yet executable
   - the operator can enable them without getting a usable runtime

2. The manager does not yet guide the operator into a working state
   - the UI explains the truth, but it does not yet offer the next best fix
   - there is no `convert`, `complete setup`, or `fix this` action

3. Runtime state is still too session-centric
   - "waits for next runtime session" is accurate, but not intuitive
   - Probe should translate this into plain operator language like:
     - "saved only"
     - "ready after restart"
     - "connected now"
     - "cannot run yet"

4. MCP execution visibility is still weaker than built-in tools
   - the user should be able to tell:
     - which MCP server supplied the tool
     - what the tool did
     - whether it succeeded or failed
     - whether approval was required

### Secondary Priority

5. There is no Probe-native conversion path from recipe to runtime entry
6. There is no support matrix inside the product
7. There is no MCP-specific doctor flow
8. There is no first-class auth/setup recovery flow for providers that need
   env vars, files, or login steps

## Product Principles

### Principle 1: Saved Is Not Ready

Probe must never let a saved recipe look equivalent to a runnable MCP.

The product language should distinguish:

- `saved recipe`
- `ready to convert`
- `manual runtime server`
- `connected now`
- `attach failed`
- `unsupported in Probe today`

### Principle 2: Always Offer The Next Fix

Every non-working MCP state should have an explicit next action.

Examples:

- recipe only -> `Convert to runtime server`
- missing launch details -> `Complete setup`
- disabled -> `Enable`
- enabled but not attached -> `Start a new turn`
- attach failed -> `Inspect error` and `Edit launch command`
- unsupported transport -> `Use stdio for now`

### Principle 3: Show Current Session Truth Separately From Saved Config

Probe must separate:

- what is saved in `PROBE_HOME`
- what is enabled in config
- what is attached to this session
- what tools are actually available right now

### Principle 4: MCP Should Feel Like A First-Class Tool Source

When the model uses an MCP tool, Probe should present it with the same clarity
as built-in tools, while still preserving origin:

- `MCP shadcn · add_component`
- `MCP filesystem · read`

## Delivery Order

### Phase A: Usability Language And State Model

Status: `[ ]`

#### Outcome

Probe uses operator language for MCP lifecycle states instead of low-level
runtime wording.

#### Scope

- [ ] define a user-facing MCP state vocabulary:
  - `saved recipe`
  - `needs conversion`
  - `ready after next turn`
  - `connected now`
  - `attach failed`
  - `unsupported`
- [ ] update `/mcp`, `/status`, and `/doctor` to use the same vocabulary
- [ ] add a visible support note for provider recipes vs manual `stdio`
- [ ] add a short help legend inside `/mcp`

#### Exit Criteria

- [ ] the operator can tell in one scan whether an MCP is usable now
- [ ] "enabled but not attached" no longer feels ambiguous

### Phase B: Recipe-To-Runtime Conversion

Status: `[ ]`

#### Outcome

A saved provider recipe can be converted into a real runtime entry from inside
Probe.

#### Scope

- [ ] add `Convert to runtime server` action for provider recipes
- [ ] prefill the conversion form from the provider recipe
- [ ] keep advanced fields hidden unless needed
- [ ] save the converted entry as a manual `stdio` runtime server
- [ ] preserve the original provider command as provenance

#### Exit Criteria

- [ ] the operator can move from provider recipe to runnable MCP without
      retyping everything by hand
- [ ] the UI makes it obvious which entry is now the runnable one

### Phase C: Known Provider Adapters

Status: `[ ]`

#### Outcome

Popular provider commands import directly into runnable Probe setups when the
mapping is known.

#### Scope

- [ ] add an adapter seam for known providers
- [ ] start with `shadcn`
- [ ] infer a Probe-friendly manual runtime entry when possible
- [ ] show an import preview before saving
- [ ] fall back safely when full conversion is not possible

#### Exit Criteria

- [ ] `pnpm dlx shadcn@latest mcp init --client codex`
      can become a Probe-manageable runtime entry through an explicit import
      flow
- [ ] unsupported commands degrade gracefully into saved recipes with next
      steps

### Phase D: MCP Doctor And Recovery Flows

Status: `[ ]`

#### Outcome

Probe can explain why an MCP is not working and guide the operator to a fix.

#### Scope

- [ ] add MCP-specific doctor cards:
  - missing command
  - launch failure
  - invalid JSON-RPC framing
  - initialize failure
  - tools/list failure
  - empty tool inventory
  - unsupported transport
- [ ] add actionable next steps per failure
- [ ] add inline `Inspect error`, `Edit`, and `Retry on next turn`

#### Exit Criteria

- [ ] an operator does not need repo knowledge or terminal debugging to
      understand why an MCP failed

### Phase E: Session And Turn Clarity

Status: `[ ]`

#### Outcome

Probe makes it obvious when MCP tools are live and when they are used.

#### Scope

- [ ] add `mcp tools: N available now` to the session rail when connected
- [ ] show the MCP source in tool call and tool result rows
- [ ] add MCP usage notes to final task receipts
- [ ] keep conversation view calm while preserving MCP provenance

#### Exit Criteria

- [ ] after a successful MCP-backed turn, the operator can clearly tell that an
      MCP tool was used and which server it came from

## Required User Flows

### Flow 1: Provider Docs To Working MCP

1. `/mcp`
2. `Add MCP server`
3. paste provider command
4. Probe classifies it as:
   - directly convertible, or
   - saved recipe needing conversion
5. operator chooses `Convert to runtime server`
6. Probe saves a runnable manual `stdio` entry
7. Probe says whether it will work now or after the next turn
8. operator starts a turn
9. Probe shows connected state and tool count

### Flow 2: Failed MCP Attach

1. operator opens `/mcp`
2. sees `attach failed`
3. opens details
4. sees:
   - what failed
   - why
   - what to do next
5. chooses `Edit` or `Retry`

### Flow 3: MCP Used In A Real Turn

1. operator asks for a task
2. model calls an MCP tool
3. transcript shows readable MCP tool provenance
4. final assistant reply stays conversational
5. receipt confirms MCP usage without dumping raw trace noise

## Edge Cases That Must Be Covered

- provider recipe saved but never converted
- converted runtime entry missing command or binary
- enabled server requires next turn to attach
- server attaches but returns zero tools
- tool inventory changes between sessions
- MCP tool requires approval and is rejected
- MCP tool returns malformed content
- server is disabled while an old session still shows prior attachment state
- multiple MCP servers expose similar tool names
- unsupported transport is configured
- operator removes a server that is attached in the current session

## Recommended Next Implementation Order

1. Phase A
   - fix the language first so users understand what they are seeing
2. Phase B
   - add `Convert to runtime server`
3. Phase D
   - add MCP doctor and failure actions
4. Phase E
   - tighten turn-time clarity and receipts
5. Phase C
   - add provider-specific adapters, starting with `shadcn`

This order keeps the product honest and immediately more usable, even before
full provider automation lands.
