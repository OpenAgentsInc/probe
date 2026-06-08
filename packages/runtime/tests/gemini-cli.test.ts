import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import { runProbeCli } from "../src";

const sse = (...events: ReadonlyArray<unknown>): string =>
  `${events.map((event) => `data: ${JSON.stringify(event)}\n`).join("\n")}data: [DONE]\n\n`;

describe("Probe CLI Gemini backend commands", () => {
  test("probe backend gemini smoke completes through the Gemini backend without exposing API keys", async () => {
    const seen = {
      url: "",
      apiKey: "",
      body: undefined as unknown,
    };
    const result = await Effect.runPromise(
      runProbeCli(["backend", "gemini", "smoke", "--prompt", "hello"], {
        env: { GOOGLE_GENERATIVE_AI_API_KEY: "test-gemini-key" },
        fetch: async (input, init) => {
          seen.url = String(input);
          seen.apiKey = new Headers(init?.headers).get("x-goog-api-key") ?? "";
          seen.body = JSON.parse(String(init?.body));
          return new Response(
            sse({
              candidates: [{ content: { role: "model", parts: [{ text: "probe gemini smoke ok" }] }, finishReason: "STOP" }],
              usageMetadata: { promptTokenCount: 2, candidatesTokenCount: 5, totalTokenCount: 7 },
            }),
            { status: 200 },
          );
        },
      }),
    );

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("Gemini smoke");
    expect(result.stdout).toContain("kind: gemini_api");
    expect(result.stdout).toContain("apiKeySource: GOOGLE_GENERATIVE_AI_API_KEY");
    expect(result.stdout).toContain("apiKeyRedacted: true");
    expect(result.stdout).toContain("assistant: probe gemini smoke ok");
    expect(result.stdout).toContain("usage: input=2 output=5 total=7");
    expect(result.stdout).not.toContain("test-gemini-key");
    expect(seen.apiKey).toBe("test-gemini-key");
    expect(seen.url).toBe(
      "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse",
    );
    expect(seen.body).toMatchObject({
      contents: [{ role: "user", parts: [{ text: "hello" }] }],
    });
  });

  test("probe backend gemini complete honors model option and PROBE_BACKEND_PROFILE", async () => {
    const urls: string[] = [];
    const result = await Effect.runPromise(
      runProbeCli(["backend", "gemini", "complete", "--model", "gemini-2.5-flash", "--prompt", "complete"], {
        env: { GEMINI_API_KEY: "test-gemini-key", PROBE_BACKEND_PROFILE: "gemini-api" },
        fetch: async (input) => {
          urls.push(String(input));
          return new Response(
            sse({
              candidates: [{ content: { role: "model", parts: [{ text: "done" }] }, finishReason: "STOP" }],
            }),
            { status: 200 },
          );
        },
      }),
    );

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("Gemini completion");
    expect(result.stdout).toContain("model: gemini-2.5-flash");
    expect(result.stdout).toContain("apiKeySource: GEMINI_API_KEY");
    expect(result.stdout).not.toContain("test-gemini-key");
    expect(urls[0]).toContain("/v1beta/models/gemini-2.5-flash:streamGenerateContent");
  });

  test("probe backend gemini smoke falls back to an authenticated Omega Gemini broker", async () => {
    const seen = {
      url: "",
      authorization: "",
      apiKey: "",
    };
    const result = await Effect.runPromise(
      runProbeCli(["backend", "gemini", "smoke", "--prompt", "hello"], {
        env: {
          PROBE_OMEGA_BASE_URL: "https://openagents.com",
          PROBE_OMEGA_BEARER_TOKEN: "oa_agent_test",
        },
        fetch: async (input, init) => {
          const headers = new Headers(init?.headers);
          seen.url = String(input);
          seen.authorization = headers.get("authorization") ?? "";
          seen.apiKey = headers.get("x-goog-api-key") ?? "";
          return new Response(
            sse({
              candidates: [{ content: { role: "model", parts: [{ text: "omega ok" }] }, finishReason: "STOP" }],
              usageMetadata: { promptTokenCount: 2, candidatesTokenCount: 1, totalTokenCount: 3 },
            }),
            { status: 200 },
          );
        },
      }),
    );

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("apiKeySource: PROBE_OMEGA_BEARER_TOKEN");
    expect(result.stdout).toContain("assistant: omega ok");
    expect(result.stdout).not.toContain("oa_agent_test");
    expect(seen.authorization).toBe("Bearer oa_agent_test");
    expect(seen.apiKey).toBe("");
    expect(seen.url).toBe(
      "https://openagents.com/api/provider-accounts/google-gemini/models/gemini-2.5-flash:streamGenerateContent?alt=sse",
    );
  });

  test("probe backend gemini smoke reports missing keys without leaking provider request details", async () => {
    const result = await Effect.runPromise(runProbeCli(["backend", "gemini", "smoke"], { env: {} }));

    expect(result.exitCode).toBe(1);
    expect(result.stderr).toContain("missing Gemini API key");
    expect(result.stderr).not.toContain("x-goog-api-key");
  });

  test("probe chat --prompt runs one Gemini turn and shows tool calls", async () => {
    const bodies: unknown[] = [];
    const responses = [
      sse({
        candidates: [
          {
            content: { role: "model", parts: [{ functionCall: { name: "current_time", args: {} } }] },
            finishReason: "STOP",
          },
        ],
      }),
      sse({
        candidates: [{ content: { role: "model", parts: [{ text: "It is test time." }] }, finishReason: "STOP" }],
        usageMetadata: { promptTokenCount: 10, candidatesTokenCount: 5, totalTokenCount: 15 },
      }),
    ];
    const result = await Effect.runPromise(
      runProbeCli(["chat", "--prompt", "what time is it?"], {
        env: { GEMINI_API_KEY: "test-gemini-key" },
        now: new Date("2026-06-08T12:00:00.000Z"),
        fetch: async (_input, init) => {
          bodies.push(JSON.parse(String(init?.body)));
          return new Response(responses.shift() ?? "", { status: 200 });
        },
      }),
    );

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("Probe Gemini chat");
    expect(result.stdout).toContain("apiKeySource: GEMINI_API_KEY");
    expect(result.stdout).toContain("apiKeyRedacted: true");
    expect(result.stdout).toContain("tool_call: current_time {}");
    expect(result.stdout).toContain("tool_result: current_time");
    expect(result.stdout).toContain("assistant: It is test time.");
    expect(result.stdout).toContain("usage: input=10 output=5 total=15");
    expect(result.stdout).not.toContain("test-gemini-key");
    expect(bodies[0]).toMatchObject({
      tools: [
        {
          functionDeclarations: expect.arrayContaining([
            expect.objectContaining({ name: "read_file" }),
            expect.objectContaining({ name: "current_time" }),
          ]),
        },
      ],
      toolConfig: { functionCallingConfig: { mode: "AUTO" } },
    });
  });

  test("probe chat without a prompt tells non-interactive callers how to start chat", async () => {
    const result = await Effect.runPromise(runProbeCli(["chat"], { env: { GEMINI_API_KEY: "test-gemini-key" } }));

    expect(result.exitCode).toBe(1);
    expect(result.stdout).toContain("Run `probe chat` for the prompt");
    expect(result.stdout).not.toContain("test-gemini-key");
  });

  test("probe chat compacts read_file tool results for terminal output", async () => {
    const responses = [
      sse({
        candidates: [
          {
            content: { role: "model", parts: [{ functionCall: { name: "read_file", args: { path: "README.md" } } }] },
            finishReason: "STOP",
          },
        ],
      }),
      sse({
        candidates: [{ content: { role: "model", parts: [{ text: "Read it." }] }, finishReason: "STOP" }],
      }),
    ];
    const result = await Effect.runPromise(
      runProbeCli(["chat", "--prompt", "read README"], {
        env: { GEMINI_API_KEY: "test-gemini-key" },
        fetch: async () => new Response(responses.shift() ?? "", { status: 200 }),
      }),
    );

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("tool_call: read_file {\"path\":\"README.md\"}");
    expect(result.stdout).toContain("tool_result: read_file {\"path\":\"README.md\",\"contentChars\":");
    expect(result.stdout).toContain("\"preview\":\"# Probe");
    expect(result.stdout.length).toBeLessThan(1600);
  });
});
