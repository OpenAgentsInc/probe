import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import { APPLE_FM_DEFAULT_MODEL_ID, runProbeCli } from "../src";

describe("Probe CLI Apple FM commands", () => {
  test("probe apple-fm status reports a ready fake bridge without inference", async () => {
    const seenMethods: string[] = [];
    const fetchImpl: typeof fetch = async (input, init) => {
      seenMethods.push(init?.method ?? "GET");
      expect(new URL(String(input)).pathname).toBe("/health");
      return Response.json({
        ready: true,
        modelId: APPLE_FM_DEFAULT_MODEL_ID,
        platform: "fake-apple-silicon",
        version: "test",
      });
    };

    const result = await Effect.runPromise(
      runProbeCli(["apple-fm", "status", "--base-url", "http://127.0.0.1:11439"], {
        fetch: fetchImpl,
        now: new Date("2026-06-07T00:00:00.000Z"),
      }),
    );

    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("status: ready");
    expect(result.stdout).toContain("kind: apple_fm_bridge");
    expect(result.stdout).toContain("platform: fake-apple-silicon");
    expect(result.stdout).toContain("\"contentRedacted\":true");
    expect(seenMethods).toEqual(["GET"]);
  });

  test("probe apple-fm status reports unsupported fake hardware as non-ready", async () => {
    const result = await Effect.runPromise(
      runProbeCli(["apple-fm", "status"], {
        fetch: async () =>
          Response.json({
            ready: false,
            modelId: APPLE_FM_DEFAULT_MODEL_ID,
            unavailableReason: "unsupported_hardware",
            message: "Apple Foundation Models are unavailable on this host.",
          }),
        now: new Date("2026-06-07T00:00:00.000Z"),
      }),
    );

    expect(result.exitCode).toBe(1);
    expect(result.stdout).toContain("status: unsupported");
    expect(result.stdout).toContain("unavailableReason: unsupported_hardware");
    expect(result.stdout).toContain("Apple Foundation Models are unavailable");
  });

  test("probe apple-fm status reports unreachable bridge without generic success", async () => {
    const result = await Effect.runPromise(
      runProbeCli(["apple-fm", "status"], {
        fetch: async () => {
          throw new Error("connection refused");
        },
        now: new Date("2026-06-07T00:00:00.000Z"),
      }),
    );

    expect(result.exitCode).toBe(1);
    expect(result.stdout).toContain("status: unreachable");
    expect(result.stdout).toContain("unavailableReason: bridge_unreachable");
    expect(result.stdout).toContain("\"ready\":false");
  });
});

