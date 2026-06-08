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

  test("probe backend gemini smoke reports missing keys without leaking provider request details", async () => {
    const result = await Effect.runPromise(runProbeCli(["backend", "gemini", "smoke"], { env: {} }));

    expect(result.exitCode).toBe(1);
    expect(result.stderr).toContain("missing Gemini API key");
    expect(result.stderr).not.toContain("x-goog-api-key");
  });
});
