import { Effect } from "effect"
import { GrantError, ResolvedInferenceGrant } from "./grant"
import type { InferenceGrantRef } from "./refs"

/**
 * A pluggable grant issuer. The primary implementation resolves a
 * Sarah-minted `sarah.inference_grant.v1` (delivered by the controller at
 * spawn); Omega and direct-key issuers are the dev fallbacks. The host
 * depends on this interface, never on a concrete provider — the whole point
 * of the funnel inversion is that probe only ever knew "one endpoint".
 */
export interface GrantIssuer {
  readonly resolve: (
    request: GrantResolveRequest,
  ) => Effect.Effect<ResolvedInferenceGrant, GrantError>
}

export interface GrantResolveRequest {
  readonly grantRef: InferenceGrantRef
  readonly runnerSessionId: string
}

/**
 * Reject a grant that is not usable BEFORE any secret is materialized:
 * status, session binding, and wall-clock expiry — the exhaustive
 * cross-check discipline from the archived grant-client, minus the fields
 * that do not survive the funnel inversion.
 */
export function assertGrantUsable(
  grant: ResolvedInferenceGrant,
  request: GrantResolveRequest,
  now: number,
): Effect.Effect<ResolvedInferenceGrant, GrantError> {
  return Effect.gen(function* () {
    if (grant.grantRef !== request.grantRef) {
      return yield* Effect.fail(new GrantError({ reason: "resolved grantRef does not match the request" }))
    }
    if (grant.runnerSessionId !== request.runnerSessionId) {
      return yield* Effect.fail(new GrantError({ reason: "grant is bound to a different runner session" }))
    }
    if (grant.status !== "active") {
      return yield* Effect.fail(new GrantError({ reason: `grant is ${grant.status}, not active` }))
    }
    if (grant.expiresAt <= now) {
      return yield* Effect.fail(new GrantError({ reason: "grant has expired" }))
    }
    if (grant.materialization.providerSecretRef !== grant.providerSecretRef) {
      return yield* Effect.fail(
        new GrantError({ reason: "materialization plan secret ref does not match the grant" }),
      )
    }
    return grant
  })
}
