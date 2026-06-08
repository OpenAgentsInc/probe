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
