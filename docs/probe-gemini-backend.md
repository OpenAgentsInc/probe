# Probe Gemini Backend

Probe registers a `gemini-api` backend profile for direct Google Gemini API
calls.

Initial profile defaults:

- kind: `gemini_api`
- attach mode: `direct_api`
- auth: `api_key`
- stream mode: `sse`
- base URL: `https://generativelanguage.googleapis.com/v1beta`
- model: `gemini-2.5-flash`

API-key resolution follows the Opencode-compatible order:

1. explicit API key option;
2. `GOOGLE_GENERATIVE_AI_API_KEY`;
3. `GEMINI_API_KEY`;
4. typed missing-credential failure.

Gemini auth receipts record only the source label and `apiKeyRedacted: true`.
They must not include the raw key or provider request headers. Runtime HTTP
code may still construct the `x-goog-api-key` header at the final request
boundary.

## Request Lowering

Gemini request lowering lives in
`packages/runtime/src/backends/gemini/protocol.ts`.

It converts the provider-neutral Probe LLM request contract into Gemini
`streamGenerateContent` bodies:

- top-level Probe system messages become Gemini `systemInstruction`;
- chronological system messages inside the transcript become wrapped user
  system-update text;
- user text and media become Gemini `user` content parts;
- assistant text, reasoning, and tool-call history become Gemini `model`
  content parts;
- tool results become Gemini `functionResponse` parts;
- Probe tool definitions become Gemini native function declarations;
- tool choice maps to Gemini function-calling modes.

Tool schemas are sanitized in
`packages/runtime/src/backends/gemini/tool-schema.ts` before they are sent to
Gemini. This avoids request-time failures for common JSON Schema shapes that
Gemini rejects, such as integer enums, dangling required fields, untyped arrays,
and scalar schemas carrying object-only keys.

## SSE Stream Parsing

`parseGeminiSseStream` converts Gemini `streamGenerateContent?alt=sse` events
into Probe LLM events. It preserves visible text, reasoning text, native
function calls, finish reason, token usage, cached-token counts, and Gemini
thought signatures as provider metadata.

Gemini reports `candidatesTokenCount` as visible output and
`thoughtsTokenCount` separately. Probe usage stores inclusive output tokens, so
the parser sums those fields when both are available.

## Native Tool Loop

`makeGeminiClient` exposes a `complete` method that sends Probe LLM requests to
Gemini and handles native function-call continuation:

1. lower the current Probe transcript into Gemini `contents`;
2. send native function declarations;
3. parse Gemini SSE events;
4. dispatch emitted function calls through Probe's provider-neutral tool
   runtime;
5. append assistant tool-call history and user `functionResponse` history;
6. repeat until Gemini returns final text or the round-trip limit is reached.

The loop uses Gemini native function declarations and responses. It does not
reuse the Apple FM callback URL bridge.
