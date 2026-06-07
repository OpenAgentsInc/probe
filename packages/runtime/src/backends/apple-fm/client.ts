import { Effect, Schema as S } from "effect";
import { type ResolvedProbeBackendProfile, type ResolveProbeBackendProfileOptions } from "../backend-profile";
import { resolveAppleFmBackendProfile, type ProbeBackendRegistryError } from "../registry";
import {
  AppleFmHealthResponse,
  type AppleFmHealthResponse,
  type AppleFmUnavailableReason,
} from "./contract";
import {
  AppleFmBackendFailureReceipt,
  makeAppleFmAvailabilityReceipt,
  makeAppleFmFailureReceipt,
  type AppleFmBackendAvailabilityReceipt,
} from "./receipts";

export const AppleFmHealthStatus = S.Literals(["ready", "unavailable", "unsupported", "malformed", "unreachable"]);
export type AppleFmHealthStatus = typeof AppleFmHealthStatus.Type;

export interface AppleFmClientOptions extends ResolveProbeBackendProfileOptions {
  readonly fetch?: typeof fetch;
  readonly now?: Date;
}

export interface AppleFmReadiness {
  readonly profile: ResolvedProbeBackendProfile;
  readonly status: AppleFmHealthStatus;
  readonly ready: boolean;
  readonly health?: AppleFmHealthResponse;
  readonly unavailableReason?: AppleFmUnavailableReason;
  readonly message?: string;
  readonly receipt: AppleFmBackendAvailabilityReceipt;
}

export interface AppleFmClient {
  readonly profile: ResolvedProbeBackendProfile;
  readonly health: () => Effect.Effect<AppleFmReadiness, never>;
  readonly requireReady: () => Effect.Effect<AppleFmReadiness, AppleFmBackendError>;
}

export class AppleFmBackendError extends S.TaggedErrorClass<AppleFmBackendError>()("AppleFmBackendError", {
  reason: S.String,
  failureClass: S.String,
  receipt: S.optional(AppleFmBackendFailureReceipt),
}) {}

export function makeAppleFmClient(
  options: AppleFmClientOptions = {},
): Effect.Effect<AppleFmClient, ProbeBackendRegistryError> {
  return Effect.gen(function* () {
    const profile = yield* resolveAppleFmBackendProfile(options);
    const fetchImpl = options.fetch ?? fetch;
    const now = () => (options.now ?? new Date()).toISOString();

    const client: AppleFmClient = {
      profile,
      health: () => checkAppleFmHealth(profile, fetchImpl, now()),
      requireReady: () =>
        checkAppleFmHealth(profile, fetchImpl, now()).pipe(
          Effect.flatMap((readiness) =>
            readiness.ready
              ? Effect.succeed(readiness)
              : Effect.fail(
                  new AppleFmBackendError({
                    reason: readiness.message ?? `Apple FM backend is ${readiness.status}`,
                    failureClass: readiness.unavailableReason ?? readiness.status,
                    receipt: makeAppleFmFailureReceipt({
                      profileId: profile.id,
                      model: profile.model,
                      baseUrl: profile.baseUrl,
                      failureClass: readiness.unavailableReason ?? readiness.status,
                      message: readiness.message ?? `Apple FM backend is ${readiness.status}`,
                      observedAt: now(),
                    }),
                  }),
                ),
          ),
        ),
    };

    return client;
  });
}

export function checkAppleFmHealth(
  profile: ResolvedProbeBackendProfile,
  fetchImpl: typeof fetch = fetch,
  observedAt = new Date().toISOString(),
): Effect.Effect<AppleFmReadiness, never> {
  return Effect.gen(function* () {
    const endpoint = new URL(profile.readinessPath, withTrailingSlash(profile.baseUrl));
    const response = yield* Effect.tryPromise({
      try: () => fetchImpl(endpoint, { method: "GET" }),
      catch: (error) =>
        unavailableReadiness(profile, {
          status: "unreachable",
          unavailableReason: "bridge_unreachable",
          message: `Apple FM bridge is unreachable: ${String(error)}`,
          observedAt,
        }),
    });

    if (isReadiness(response)) {
      return response;
    }

    if (!response.ok) {
      return unavailableReadiness(profile, {
        status: "unavailable",
        unavailableReason: "not_ready",
        message: `Apple FM bridge health returned HTTP ${response.status}`,
        observedAt,
      });
    }

    const raw = yield* Effect.tryPromise({
      try: () => response.json(),
      catch: (error) =>
        unavailableReadiness(profile, {
          status: "malformed",
          unavailableReason: "malformed_response",
          message: `Apple FM bridge health response was not JSON: ${String(error)}`,
          observedAt,
        }),
    });

    if (isReadiness(raw)) {
      return raw;
    }

    const decoded = yield* S.decodeUnknownEffect(AppleFmHealthResponse)(raw).pipe(
      Effect.mapError((error) =>
        unavailableReadiness(profile, {
          status: "malformed",
          unavailableReason: "malformed_response",
          message: `Apple FM bridge health response was malformed: ${String(error)}`,
          observedAt,
        }),
      ),
    );

    if (isReadiness(decoded)) {
      return decoded;
    }

    const model = decoded.modelId ?? decoded.model ?? profile.model;
    const ready = decoded.ready === true;
    const unavailableReason = decoded.unavailableReason;
    const status = ready ? "ready" : healthStatusFromReason(unavailableReason);

    return {
      profile,
      status,
      ready,
      health: decoded,
      unavailableReason,
      message: decoded.message,
      receipt: makeAppleFmAvailabilityReceipt({
        profileId: profile.id,
        model,
        baseUrl: profile.baseUrl,
        ready,
        unavailableReason,
        message: decoded.message,
        observedAt,
      }),
    };
  }).pipe(Effect.catch((readiness: AppleFmReadiness) => Effect.succeed(readiness)));
}

function unavailableReadiness(
  profile: ResolvedProbeBackendProfile,
  input: {
    readonly status: Exclude<AppleFmHealthStatus, "ready">;
    readonly unavailableReason: AppleFmUnavailableReason;
    readonly message: string;
    readonly observedAt: string;
  },
): AppleFmReadiness {
  return {
    profile,
    status: input.status,
    ready: false,
    unavailableReason: input.unavailableReason,
    message: input.message,
    receipt: makeAppleFmAvailabilityReceipt({
      profileId: profile.id,
      model: profile.model,
      baseUrl: profile.baseUrl,
      ready: false,
      unavailableReason: input.unavailableReason,
      message: input.message,
      observedAt: input.observedAt,
    }),
  };
}

function healthStatusFromReason(reason: AppleFmUnavailableReason | undefined): AppleFmHealthStatus {
  if (reason === "unsupported_hardware" || reason === "apple_intelligence_disabled") {
    return "unsupported";
  }

  if (reason === "malformed_response") {
    return "malformed";
  }

  if (reason === "bridge_unreachable") {
    return "unreachable";
  }

  return "unavailable";
}

function isReadiness(value: unknown): value is AppleFmReadiness {
  return typeof value === "object" && value !== null && "profile" in value && "receipt" in value && "status" in value;
}

function withTrailingSlash(value: string): string {
  return value.endsWith("/") ? value : `${value}/`;
}
