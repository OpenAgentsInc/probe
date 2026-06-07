# Probe Blueprint Consumer Contracts

Date: 2026-06-07

Status: implemented as the first local contract layer for Probe issue #172.

Probe now carries a narrowed Effect Schema mirror for the Blueprint contracts it
needs before Omega exposes live HTTP routes. This is intentionally a consumer
surface, not a fork of the Omega Blueprint runtime. Probe does not import
`autopilot-omega`; it mirrors only the public/operator-safe fields needed for
signature lookup, registry projection decoding, backend-independent tool menu
planning, Program Run evidence flags, release gate references, and contract
export discovery.

The current fixture in `packages/runtime/src/blueprint/fixtures.ts` seeds two
Probe-facing Blueprint signatures:

- `program_signature.probe.signature_lookup.v1`
- `program_signature.probe.tool_menu.project.v1`

Both fixtures preserve Omega's intended safety posture: registry entries are
`safeProjection: true`, Program Types have `directMutationAllowed: false`, run
details are evidence-only, release gates cannot self-promote, and fixture data
uses refs instead of raw prompts, callback URLs, provider payloads, secrets,
wallet material, private repo content, or customer data.

This local mirror is temporary infrastructure for the next steps. Issue #173
will add registry sources for fixture, assignment-carried projection, and Omega
HTTP. Once Omega ships `GET /api/blueprint/program-registry` and
`GET /api/blueprint/contracts`, Probe should use those routes as the source of
truth and keep the static fixture only for tests, offline development, and
emergency bootstrap.
