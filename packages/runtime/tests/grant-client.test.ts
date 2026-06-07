import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import {
  CHATGPT_CODEX_PROVIDER,
  decodeProbeRunAssignment,
  makeOmegaGrantResolver,
  makeStaticOmegaGrantResolver,
  validateResolvedAuthGrantForAssignment,
  type OmegaResolvedAuthGrant,
  type ProbeRunAssignment,
} from "../src";

const assignment = (): ProbeRunAssignment => ({
  assignmentId: "assignment_1",
  runnerSessionId: "runner_session_1",
  goal: "Fix the failing test",
  provider: CHATGPT_CODEX_PROVIDER,
  providerAccountRef: "provider-account_primary" as ProbeRunAssignment["providerAccountRef"],
  authGrantRef: "provider-auth-grant_1" as ProbeRunAssignment["authGrantRef"],
  repo: {
    url: "https://github.com/OpenAgentsInc/probe.git",
    branch: "main",
  },
});

const grant = (overrides: Partial<OmegaResolvedAuthGrant> = {}): OmegaResolvedAuthGrant => ({
  grantRef: "provider-auth-grant_1" as OmegaResolvedAuthGrant["grantRef"],
  provider: CHATGPT_CODEX_PROVIDER,
  providerAccountRef: "provider-account_primary" as OmegaResolvedAuthGrant["providerAccountRef"],
  providerSecretRef: "codex-auth://provider-account_primary" as OmegaResolvedAuthGrant["providerSecretRef"],
  requestedAction: "coding-agent-run",
  runnerSessionId: "runner_session_1",
  expiresAt: "2099-01-01T00:00:00.000Z",
  status: "used",
  materialization: {
    kind: "probe_chatgpt_auth",
    provider: CHATGPT_CODEX_PROVIDER,
    providerSecretRef: "codex-auth://provider-account_primary" as OmegaResolvedAuthGrant["providerSecretRef"],
    target: {
      kind: "env",
      name: "PROBE_CHATGPT_AUTH_CONTENT",
    },
    homeIsolation: "per_run",
    scrubAfterCloseout: true,
  },
  ...overrides,
});

describe("Omega grant resolution", () => {
  test("parses Probe run assignments carrying provider refs and grants", async () => {
    const parsed = await Effect.runPromise(decodeProbeRunAssignment(assignment()));

    expect(parsed.providerAccountRef).toBe("provider-account_primary");
    expect(parsed.authGrantRef).toBe("provider-auth-grant_1");
    expect(parsed.runnerSessionId).toBe("runner_session_1");
  });

  test("resolves a fake Omega grant into a Probe materialization plan", async () => {
    const resolver = makeStaticOmegaGrantResolver(grant());
    const resolved = await Effect.runPromise(resolver.resolveGrant(assignment()));

    expect(resolved.materialization.kind).toBe("probe_chatgpt_auth");
    expect(resolved.materialization.target).toEqual({
      kind: "env",
      name: "PROBE_CHATGPT_AUTH_CONTENT",
    });
  });

  test("rejects mismatched provider account refs", async () => {
    await expect(
      Effect.runPromise(
        validateResolvedAuthGrantForAssignment(
          grant({
            providerAccountRef: "provider-account_backup" as OmegaResolvedAuthGrant["providerAccountRef"],
          }),
          assignment(),
        ),
      ),
    ).rejects.toMatchObject({
      _tag: "ProbeAuthGrantMismatch",
      field: "providerAccountRef",
    });
  });

  test("rejects expired grants", async () => {
    await expect(
      Effect.runPromise(
        validateResolvedAuthGrantForAssignment(
          grant({
            expiresAt: "2000-01-01T00:00:00.000Z",
          }),
          assignment(),
        ),
      ),
    ).rejects.toMatchObject({ _tag: "ProbeAuthGrantExpired" });
  });

  test("rejects used grant records that are not resolved materialization payloads", async () => {
    await expect(
      Effect.runPromise(
        validateResolvedAuthGrantForAssignment(
          {
            ...grant(),
            materialization: undefined,
          },
          assignment(),
        ),
      ),
    ).rejects.toMatchObject({ _tag: "ProbeAuthGrantResolveError" });
  });

  test("rejects materialization payloads with OpenCode env names", async () => {
    await expect(
      Effect.runPromise(
        validateResolvedAuthGrantForAssignment(
          {
            ...grant(),
            materialization: {
              ...grant().materialization,
              target: {
                kind: "env",
                name: "OPENCODE_AUTH_CONTENT",
              },
            },
          },
          assignment(),
        ),
      ),
    ).rejects.toMatchObject({ _tag: "ProbeAuthGrantResolveError" });
  });

  test("reports unavailable Omega instead of leaking assignment data", async () => {
    const resolver = makeOmegaGrantResolver({
      baseUrl: "https://omega.invalid",
      fetch: async () => new Response("unavailable", { status: 503 }),
    });

    await expect(Effect.runPromise(resolver.resolveGrant(assignment()))).rejects.toMatchObject({
      _tag: "ProbeAuthGrantResolveError",
      statusCode: 503,
    });
  });
});
