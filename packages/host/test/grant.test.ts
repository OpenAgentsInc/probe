import { describe, expect, test } from "bun:test"
import { existsSync, mkdtempSync, readFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Effect, Exit } from "effect"
import {
  assertGrantUsable,
  GrantError,
  InferenceGrantRef,
  literalBroker,
  ProviderAccountRef,
  ProviderSecretRef,
  ResolvedInferenceGrant,
  withMaterializedGrant
} from "../src"

const grant = (overrides: Partial<ResolvedInferenceGrant> = {}): ResolvedInferenceGrant => ({
  provider: "sarah_proxy",
  grantRef: InferenceGrantRef.make("grant_1"),
  providerAccountRef: ProviderAccountRef.make("account_1"),
  providerSecretRef: ProviderSecretRef.make("secret_1"),
  runnerSessionId: "session_1",
  inferenceUrl: "https://openagents.com/api/inference/proxy",
  status: "active",
  expiresAt: 10_000,
  budget: { maxTokens: 100_000, maxCalls: 16, wallClockMs: 600_000 },
  materialization: {
    providerSecretRef: ProviderSecretRef.make("secret_1"),
    target: { kind: "env", name: "PROBE_INFERENCE_GRANT" }
  },
  ...overrides
})

const request = { grantRef: InferenceGrantRef.make("grant_1"), runnerSessionId: "session_1" }

describe("inference grant validation", () => {
  test("accepts a usable grant", async () => {
    const result = await Effect.runPromise(assertGrantUsable(grant(), request, 5_000))
    expect(result.grantRef).toBe(InferenceGrantRef.make("grant_1"))
  })

  test("rejects expired, revoked, mismatched-session, and mismatched-ref grants", async () => {
    const cases: ReadonlyArray<readonly [ResolvedInferenceGrant, number, string]> = [
      [grant(), 20_000, "expired"],
      [grant({ status: "revoked" }), 5_000, "revoked"],
      [grant({ runnerSessionId: "other" }), 5_000, "different runner session"],
      [
        grant({ materialization: { providerSecretRef: ProviderSecretRef.make("secret_2"), target: { kind: "env", name: "X" } } }),
        5_000,
        "does not match the grant"
      ]
    ]
    for (const [value, now, needle] of cases) {
      const error = await Effect.runPromise(Effect.flip(assertGrantUsable(value, request, now)))
      expect(error.reason).toContain(needle)
    }
  })
})

describe("grant materialization", () => {
  test("env target injects grant + url and needs no scrub", async () => {
    const injected = await Effect.runPromise(
      withMaterializedGrant(
        { grant: grant(), runHome: mkdtempSync(join(tmpdir(), "probe-run-")), broker: literalBroker("grant_token_value") },
        (materialized) => Effect.succeed(materialized.env)
      )
    )
    expect(injected["PROBE_INFERENCE_GRANT"]).toBe("grant_token_value")
    expect(injected["PROBE_INFERENCE_URL"]).toBe("https://openagents.com/api/inference/proxy")
  })

  test("file target writes 0600, then overwrites and unlinks on success", async () => {
    const runHome = mkdtempSync(join(tmpdir(), "probe-run-"))
    let observedPath = ""
    let observedContent = ""
    await Effect.runPromise(
      withMaterializedGrant(
        {
          grant: grant({
            materialization: {
              providerSecretRef: ProviderSecretRef.make("secret_1"),
              target: { kind: "file", relativePath: "auth/token" }
            }
          }),
          runHome,
          broker: literalBroker("grant_token_value")
        },
        (materialized) =>
          Effect.sync(() => {
            observedPath = materialized.filePath as string
            observedContent = readFileSync(observedPath, "utf8")
          })
      )
    )
    expect(observedContent).toBe("grant_token_value")
    // Scrubbed after use.
    expect(existsSync(observedPath)).toBe(false)
  })

  test("scrub runs even when use fails", async () => {
    const runHome = mkdtempSync(join(tmpdir(), "probe-run-"))
    let observedPath = ""
    const exit = await Effect.runPromiseExit(
      withMaterializedGrant(
        {
          grant: grant({
            materialization: {
              providerSecretRef: ProviderSecretRef.make("secret_1"),
              target: { kind: "file", relativePath: "auth/token" }
            }
          }),
          runHome,
          broker: literalBroker("grant_token_value")
        },
        (materialized) => {
          observedPath = materialized.filePath as string
          return Effect.fail(new GrantError({ reason: "delegation failed mid-run" }))
        }
      )
    )
    expect(Exit.isFailure(exit)).toBe(true)
    expect(existsSync(observedPath)).toBe(false)
  })

  test("refuses a materialization path that escapes the run home", async () => {
    const error = await Effect.runPromise(
      Effect.flip(
        withMaterializedGrant(
          {
            grant: grant({
              materialization: {
                providerSecretRef: ProviderSecretRef.make("secret_1"),
                target: { kind: "file", relativePath: "../escape" }
              }
            }),
            runHome: mkdtempSync(join(tmpdir(), "probe-run-")),
            broker: literalBroker("grant_token_value")
          },
          () => Effect.void
        )
      )
    )
    expect((error as GrantError).reason).toContain("escapes the run home")
  })
})
