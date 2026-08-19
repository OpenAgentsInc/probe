import { Schema as S } from "effect"

/**
 * Branded refs, salvaged from the archived provider-account contract: a
 * secret ref cannot be passed where an account or grant ref is expected, so
 * the credential graph is type-checked, not stringly-typed.
 */
export const ProviderAccountRef = S.String.pipe(S.brand("ProviderAccountRef"))
export type ProviderAccountRef = typeof ProviderAccountRef.Type

export const InferenceGrantRef = S.String.pipe(S.brand("InferenceGrantRef"))
export type InferenceGrantRef = typeof InferenceGrantRef.Type

export const ProviderSecretRef = S.String.pipe(S.brand("ProviderSecretRef"))
export type ProviderSecretRef = typeof ProviderSecretRef.Type

/** The proxy providers a Sarah inference grant can authorize. */
export const GrantProvider = S.Literals(["sarah_proxy", "openai_compatible", "gemini"])
export type GrantProvider = typeof GrantProvider.Type
