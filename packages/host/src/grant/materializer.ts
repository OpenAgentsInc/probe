import { mkdir, rm, writeFile } from "node:fs/promises"
import { dirname, isAbsolute, resolve, sep } from "node:path"
import { Effect, Schema as S } from "effect"
import { GrantError, ResolvedInferenceGrant } from "./grant"

/**
 * Materialize a grant's secret into a per-run env var or 0600 file, then
 * scrub it — salvaged from the archived materializer, retargeted to the
 * inference grant and hardened at the one noted gap: files are OVERWRITTEN
 * before unlink, not merely deleted.
 *
 * Lifetime is bracketed with acquireUseRelease so the scrub runs on
 * success, failure, AND interruption — a stale process can never leave a
 * secret on disk. Receipts carry contentRedacted: true so redaction is a
 * type obligation, not a convention.
 */
export const MaterializedReceipt = S.Struct({
  kind: S.Literal("probe_grant_materialized"),
  provider: S.String,
  providerSecretRef: S.String,
  targetKind: S.Literals(["env", "file"]),
  envName: S.optional(S.String),
  relativePath: S.optional(S.String),
  contentRedacted: S.Literal(true)
})
export type MaterializedReceipt = typeof MaterializedReceipt.Type

export interface MaterializedGrant {
  readonly grant: ResolvedInferenceGrant
  /** Env to inject into the probe process (grant + endpoint URL). */
  readonly env: Readonly<Record<string, string>>
  readonly filePath?: string
  readonly receipt: MaterializedReceipt
}

/**
 * A broker turns a secret ref into bytes. In production the controller
 * already holds the grant token from the delegation event, so the default
 * broker just returns it; the interface exists so the file path and the
 * archived Omega broker both fit.
 */
export interface SecretBroker {
  readonly resolve: (ref: string) => Effect.Effect<string, GrantError>
}

export const literalBroker = (secret: string): SecretBroker => ({
  resolve: () => Effect.succeed(secret)
})

/** Reject a run-relative path that escapes the run home (archived guard). */
function resolveRunRelativePath(runHome: string, relative: string): Effect.Effect<string, GrantError> {
  if (isAbsolute(relative)) {
    return Effect.fail(new GrantError({ reason: "materialization path must be run-relative" }))
  }
  const absolute = resolve(runHome, relative)
  if (absolute !== runHome && !absolute.startsWith(runHome + sep)) {
    return Effect.fail(new GrantError({ reason: "materialization path escapes the run home" }))
  }
  return Effect.succeed(absolute)
}

async function overwriteAndUnlink(path: string, byteLength: number): Promise<void> {
  // Overwrite before unlink so the bytes do not linger on disk — the gap
  // the archived scrub (rm only) left open.
  try {
    await writeFile(path, " ".repeat(Math.max(byteLength, 1)), { mode: 0o600 })
  } catch {
    // Best effort; the unlink below is the guarantee that matters.
  }
  await rm(path, { force: true })
}

export interface MaterializeInput {
  readonly grant: ResolvedInferenceGrant
  readonly runHome: string
  readonly broker: SecretBroker
  /** Env name for the grant token itself (default PROBE_INFERENCE_GRANT). */
  readonly grantEnvName?: string
  readonly urlEnvName?: string
}

/**
 * Materialize, hand the placement to `use`, and always scrub. The `use`
 * callback receives the env/file the probe process should launch with.
 */
export function withMaterializedGrant<A, E>(
  input: MaterializeInput,
  use: (materialized: MaterializedGrant) => Effect.Effect<A, E>
): Effect.Effect<A, E | GrantError> {
  const grantEnvName = input.grantEnvName ?? "PROBE_INFERENCE_GRANT"
  const urlEnvName = input.urlEnvName ?? "PROBE_INFERENCE_URL"

  return Effect.gen(function* () {
    const secret = yield* input.broker.resolve(input.grant.providerSecretRef)
    const target = input.grant.materialization.target

    return yield* Effect.acquireUseRelease(
      Effect.gen(function* () {
        if (target.kind === "env") {
          const materialized: MaterializedGrant = {
            grant: input.grant,
            env: { [grantEnvName]: secret, [urlEnvName]: input.grant.inferenceUrl },
            receipt: {
              kind: "probe_grant_materialized",
              provider: input.grant.provider,
              providerSecretRef: input.grant.providerSecretRef,
              targetKind: "env",
              envName: grantEnvName,
              contentRedacted: true
            }
          }
          return materialized
        }
        const filePath = yield* resolveRunRelativePath(input.runHome, target.relativePath)
        yield* Effect.tryPromise({
          try: () => mkdir(dirname(filePath), { recursive: true }),
          catch: (error) => new GrantError({ reason: `mkdir failed: ${String(error)}` })
        })
        yield* Effect.tryPromise({
          try: () => writeFile(filePath, secret, { mode: 0o600 }),
          catch: (error) => new GrantError({ reason: `write failed: ${String(error)}` })
        })
        const materialized: MaterializedGrant = {
          grant: input.grant,
          env: { [urlEnvName]: input.grant.inferenceUrl },
          filePath,
          receipt: {
            kind: "probe_grant_materialized",
            provider: input.grant.provider,
            providerSecretRef: input.grant.providerSecretRef,
            targetKind: "file",
            relativePath: target.relativePath,
            contentRedacted: true
          }
        }
        return materialized
      }),
      use,
      (materialized) =>
        materialized.filePath === undefined
          ? Effect.void
          : Effect.promise(() => overwriteAndUnlink(materialized.filePath as string, secret.length))
    )
  })
}
