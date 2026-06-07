import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import { APPLE_FM_DEFAULT_MODEL_ID, makeAppleFmClient } from "../src";

describe("Apple FM snapshot streaming", () => {
  test("emits replacement snapshots and a separate final commit", async () => {
    const client = await Effect.runPromise(
      makeAppleFmClient({
        explicitBaseUrl: "http://127.0.0.1:11439",
        fetch: async (input, init) => {
          const url = new URL(String(input));
          expect(url.pathname).toBe("/v1/chat/completions");
          expect(init?.method).toBe("POST");
          const body = JSON.parse(String(init?.body));
          expect(body.stream).toBe(true);
          expect(body.streamMode).toBe("snapshot");

          return new Response(
            [
              JSON.stringify({ sequence: 0, content: "partial answer" }),
              JSON.stringify({ sequence: 1, content: "complete answer", finish_reason: "stop" }),
            ].join("\n"),
            {
              headers: {
                "Content-Type": "application/x-ndjson",
              },
            },
          );
        },
        now: new Date("2026-06-07T00:00:00.000Z"),
      }),
    );
    const result = await Effect.runPromise(
      client.streamPlainTextSnapshots([{ role: "user", content: "stream a response" }]),
    );

    expect(result.snapshots.map((snapshot) => snapshot.content)).toEqual(["partial answer", "complete answer"]);
    expect(result.completion.text).toBe("complete answer");
    expect(result.events.map((event) => event.kind)).toEqual([
      "assistant_stream_started",
      "assistant_snapshot",
      "assistant_snapshot",
      "assistant_stream_finished",
      "assistant_final_commit",
    ]);
    expect(result.events.some((event) => event.kind === "assistant_final_commit" && event.receipt !== undefined)).toBe(true);
    expect(JSON.stringify(result.events)).not.toContain("token_delta");
  });

  test("does not accumulate snapshots as deltas", async () => {
    const client = await Effect.runPromise(
      makeAppleFmClient({
        fetch: async () =>
          new Response(
            JSON.stringify([
              { sequence: 0, content: "alpha" },
              { sequence: 1, content: "alphabet" },
              { sequence: 2, content: "alphabet soup" },
            ]),
          ),
        now: new Date("2026-06-07T00:00:00.000Z"),
      }),
    );
    const result = await Effect.runPromise(client.streamPlainTextSnapshots([{ role: "user", content: "stream" }]));
    const rendered = result.events
      .filter((event) => event.kind === "assistant_snapshot")
      .reduce((_, event) => event.content ?? "", "");

    expect(rendered).toBe("alphabet soup");
    expect(rendered).not.toBe("alphaalphabetalphabet soup");
    expect(result.completion.text).toBe("alphabet soup");
  });

  test("snapshot stream failures emit typed receipts without final commit", async () => {
    const client = await Effect.runPromise(
      makeAppleFmClient({
        fetch: async () =>
          Response.json(
            {
              error: {
                message: "stream refused",
              },
            },
            { status: 503 },
          ),
        now: new Date("2026-06-07T00:00:00.000Z"),
      }),
    );

    await expect(
      Effect.runPromise(client.streamPlainTextSnapshots([{ role: "user", content: "stream" }])),
    ).rejects.toMatchObject({
      _tag: "AppleFmBackendError",
      failureClass: "stream_http_503",
      receipt: {
        kind: "probe_backend_failure",
        contentRedacted: true,
      },
    });
  });
});

