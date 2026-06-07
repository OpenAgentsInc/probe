import { Effect, Schema as S } from "effect";
import {
  APPLE_FM_BACKEND_KIND,
  APPLE_FM_DEFAULT_BASE_URL,
  APPLE_FM_DEFAULT_MODEL_ID,
  APPLE_FM_LOCAL_PROFILE_ID,
} from "./apple-fm/contract";
import { type ProbeBackendProfile, type ResolvedProbeBackendProfile, type ResolveProbeBackendProfileOptions } from "./backend-profile";

export const APPLE_FM_LOCAL_PROFILE: ProbeBackendProfile = {
  id: APPLE_FM_LOCAL_PROFILE_ID,
  kind: APPLE_FM_BACKEND_KIND,
  defaultBaseUrl: APPLE_FM_DEFAULT_BASE_URL,
  model: APPLE_FM_DEFAULT_MODEL_ID,
  attachMode: "attach_existing",
  auth: "none",
  readinessPath: "/health",
  streamMode: "snapshot",
};

export const DEFAULT_BACKEND_PROFILES: ReadonlyArray<ProbeBackendProfile> = [APPLE_FM_LOCAL_PROFILE];

export class ProbeBackendRegistryError extends S.TaggedErrorClass<ProbeBackendRegistryError>()(
  "ProbeBackendRegistryError",
  {
    reason: S.String,
  },
) {}

export function lookupBackendProfile(
  profileId: string,
  profiles: ReadonlyArray<ProbeBackendProfile> = DEFAULT_BACKEND_PROFILES,
): Effect.Effect<ProbeBackendProfile, ProbeBackendRegistryError> {
  const profile = profiles.find((candidate) => candidate.id === profileId);

  return profile === undefined
    ? Effect.fail(new ProbeBackendRegistryError({ reason: `unknown backend profile: ${profileId}` }))
    : Effect.succeed(profile);
}

export function resolveBackendProfile(
  options: ResolveProbeBackendProfileOptions = {},
  profiles: ReadonlyArray<ProbeBackendProfile> = DEFAULT_BACKEND_PROFILES,
): Effect.Effect<ResolvedProbeBackendProfile, ProbeBackendRegistryError> {
  return Effect.gen(function* () {
    const profile = yield* lookupBackendProfile(options.profileId ?? APPLE_FM_LOCAL_PROFILE_ID, profiles);
    const resolvedBaseUrl = resolveAppleFmBaseUrl(profile.defaultBaseUrl, options);

    return {
      ...profile,
      baseUrl: resolvedBaseUrl.baseUrl,
      baseUrlSource: resolvedBaseUrl.baseUrlSource,
    };
  });
}

export function resolveAppleFmBackendProfile(
  options: ResolveProbeBackendProfileOptions = {},
): Effect.Effect<ResolvedProbeBackendProfile, ProbeBackendRegistryError> {
  return resolveBackendProfile({ ...options, profileId: options.profileId ?? APPLE_FM_LOCAL_PROFILE_ID });
}

function resolveAppleFmBaseUrl(
  defaultBaseUrl: string,
  options: ResolveProbeBackendProfileOptions,
): Pick<ResolvedProbeBackendProfile, "baseUrl" | "baseUrlSource"> {
  if (isNonEmptyString(options.explicitBaseUrl)) {
    return { baseUrl: options.explicitBaseUrl, baseUrlSource: "explicit" };
  }

  if (isNonEmptyString(options.env?.PROBE_APPLE_FM_BASE_URL)) {
    return { baseUrl: options.env.PROBE_APPLE_FM_BASE_URL, baseUrlSource: "PROBE_APPLE_FM_BASE_URL" };
  }

  if (isNonEmptyString(options.env?.OPENAGENTS_APPLE_FM_BASE_URL)) {
    return { baseUrl: options.env.OPENAGENTS_APPLE_FM_BASE_URL, baseUrlSource: "OPENAGENTS_APPLE_FM_BASE_URL" };
  }

  return { baseUrl: defaultBaseUrl, baseUrlSource: "default" };
}

function isNonEmptyString(value: string | undefined): value is string {
  return value !== undefined && value.trim().length > 0;
}
