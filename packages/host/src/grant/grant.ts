import { Schema as S } from "effect"
import { GrantProvider, InferenceGrantRef, ProviderAccountRef, ProviderSecretRef } from "./refs"

/**
 * A resolved inference grant: authority to call ONE endpoint for ONE
 * delegation, budgeted and generation-fenced. It carries a materialization
 * plan, never the secret itself — the secret is resolved separately by a
 * broker, exactly as the archived Omega grant model did.
 */
export const GrantBudget = S.Struct({
  maxTokens: S.optional(S.Number),
  maxCalls: S.optional(S.Number),
  wallClockMs: S.optional(S.Number),
})
export type GrantBudget = typeof GrantBudget.Type

export const GrantMaterializationTarget = S.Union([
  S.Struct({ kind: S.Literal("env"), name: S.String }),
  S.Struct({ kind: S.Literal("file"), relativePath: S.String }),
])
export type GrantMaterializationTarget = typeof GrantMaterializationTarget.Type

export const GrantStatus = S.Literals(["active", "expired", "revoked", "failed"])
export type GrantStatus = typeof GrantStatus.Type

export const ResolvedInferenceGrant = S.Struct({
  provider: GrantProvider,
  grantRef: InferenceGrantRef,
  providerAccountRef: ProviderAccountRef,
  providerSecretRef: ProviderSecretRef,
  runnerSessionId: S.String,
  /** The endpoint probe's transport must target. */
  inferenceUrl: S.String,
  status: GrantStatus,
  /** Unix ms; a grant past this is rejected before materialization. */
  expiresAt: S.Number,
  budget: GrantBudget,
  materialization: S.Struct({
    providerSecretRef: ProviderSecretRef,
    target: GrantMaterializationTarget,
  }),
})
export type ResolvedInferenceGrant = typeof ResolvedInferenceGrant.Type

export class GrantError extends S.TaggedErrorClass<GrantError>()("GrantError", {
  reason: S.String,
}) {}
