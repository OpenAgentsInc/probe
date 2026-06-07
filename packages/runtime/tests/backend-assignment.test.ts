import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import {
  APPLE_FM_DEFAULT_MODEL_ID,
  PROBE_APPLE_FM_BACKEND_CAPABILITY,
  runProbeBackendAssignment,
  type ProbeRunAssignment,
  type ProbeRunnerAssignmentProof,
  type ProbeRunnerIdentity,
} from "../src";

const assignment = (): ProbeRunAssignment => ({
  assignmentId: "assignment_apple_fm_1",
  runnerSessionId: "runner_session_1",
  goal: "Summarize this local repo.",
  backend: {
    kind: "apple_fm_bridge",
    profile: "apple-fm-local",
  },
});

const runner = (capabilities = ["probe.run", PROBE_APPLE_FM_BACKEND_CAPABILITY]): ProbeRunnerIdentity => ({
  runnerId: "runner_local_1",
  kind: "local",
  linkedSubject: "user_1",
  linkedAt: "2026-06-07T00:00:00.000Z",
  capabilities,
});

const proof = (): ProbeRunnerAssignmentProof => ({
  runnerId: "runner_local_1",
  assignmentId: "assignment_apple_fm_1",
  runnerSessionId: "runner_session_1",
  issuedAt: "2026-06-07T00:00:00.000Z",
  nonce: "nonce_apple_fm_1",
  proofKind: "test",
});

describe("Probe backend assignment routing", () => {
  test("runs an Apple FM assignment without provider auth materialization", async () => {
    const seenPaths: string[] = [];
    const result = await Effect.runPromise(
      runProbeBackendAssignment({
        runner: runner(),
        proof: proof(),
        assignment: assignment(),
        trustedBackendBaseUrl: "http://127.0.0.1:11439",
        fetch: async (input, init) => {
          const url = new URL(String(input));
          seenPaths.push(`${init?.method ?? "GET"} ${url.pathname}`);

          if (url.pathname === "/health") {
            return Response.json({
              ready: true,
              modelId: APPLE_FM_DEFAULT_MODEL_ID,
            });
          }

          const body = JSON.parse(String(init?.body));
          expect(body.messages[0].content).toBe("Summarize this local repo.");
          return Response.json({
            model: APPLE_FM_DEFAULT_MODEL_ID,
            choices: [
              {
                message: {
                  role: "assistant",
                  content: "local summary",
                },
                finish_reason: "stop",
              },
            ],
            usage: {
              truth: "exact",
              promptTokens: 4,
              completionTokens: 2,
              totalTokens: 6,
            },
          });
        },
        now: new Date("2026-06-07T00:00:00.000Z"),
      }),
    );

    expect(result.authRequired).toBe(false);
    expect(result.backendKind).toBe("apple_fm_bridge");
    expect(result.completion.text).toBe("local summary");
    expect(result.completion.usage.truth).toBe("exact");
    expect(result.events.map((event) => event.kind)).toEqual([
      "probe_backend_run_started",
      "probe_backend_run_finished",
    ]);
    expect(seenPaths).toEqual(["GET /health", "POST /v1/chat/completions"]);
  });

  test("rejects Apple FM assignment when runner lacks backend capability", async () => {
    await expect(
      Effect.runPromise(
        runProbeBackendAssignment({
          runner: runner(["probe.run"]),
          proof: proof(),
          assignment: assignment(),
          fetch: async () => Response.json({ ready: true, modelId: APPLE_FM_DEFAULT_MODEL_ID }),
        }),
      ),
    ).rejects.toMatchObject({ _tag: "ProbeRunnerAuthorizationError" });
  });

  test("rejects Apple FM assignment when live health is not ready", async () => {
    await expect(
      Effect.runPromise(
        runProbeBackendAssignment({
          runner: runner(),
          proof: proof(),
          assignment: assignment(),
          fetch: async () =>
            Response.json({
              ready: false,
              modelId: APPLE_FM_DEFAULT_MODEL_ID,
              unavailableReason: "model_unavailable",
              message: "model is not admitted on this host",
            }),
          now: new Date("2026-06-07T00:00:00.000Z"),
        }),
      ),
    ).rejects.toMatchObject({
      _tag: "ProbeBackendAssignmentError",
      receipt: {
        kind: "probe_backend_availability",
        ready: false,
        unavailableReason: "model_unavailable",
      },
      events: [
        { kind: "probe_backend_run_started" },
        { kind: "probe_backend_run_failed" },
      ],
    });
  });
});

