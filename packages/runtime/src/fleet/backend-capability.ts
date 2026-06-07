import { Effect, Schema as S } from "effect";
import { makeAppleFmClient } from "../backends/apple-fm/client";
import { APPLE_FM_BACKEND_KIND } from "../backends/apple-fm/contract";
import { redactUrl } from "../backends/apple-fm/receipts";
import { PROBE_APPLE_FM_BACKEND_CAPABILITY, type ProbeRunnerIdentity } from "../runner/identity";

export const ProbeBackendCapabilityReport = S.Struct({
  kind: S.Literal("probe_backend_capability_report"),
  runnerId: S.String,
  runnerKind: S.Literals(["local", "shc", "pylon", "sandbox"]),
  backendKind: S.Literal(APPLE_FM_BACKEND_KIND),
  profileId: S.String,
  model: S.String,
  capability: S.Literal(PROBE_APPLE_FM_BACKEND_CAPABILITY),
  advertisedCapabilities: S.Array(S.String),
  available: S.Boolean,
  status: S.Literals(["ready", "unavailable", "unsupported", "malformed", "unreachable"]),
  baseUrl: S.String,
  platform: S.optional(S.String),
  version: S.optional(S.String),
  unavailableReason: S.optional(S.String),
  message: S.optional(S.String),
  requirements: S.Struct({
    appleSilicon: S.Literal("required"),
    appleIntelligence: S.Literal("required"),
    liveHealth: S.Literal("required"),
  }),
  support: S.Struct({
    snapshotStreaming: S.Boolean,
    toolCallbacks: S.Boolean,
  }),
  receipt: S.Unknown,
  observedAt: S.String,
  contentRedacted: S.Literal(true),
});
export type ProbeBackendCapabilityReport = typeof ProbeBackendCapabilityReport.Type;

export interface ReportAppleFmBackendCapabilityInput {
  readonly runner: ProbeRunnerIdentity;
  readonly trustedBackendBaseUrl?: string;
  readonly env?: Readonly<Record<string, string | undefined>>;
  readonly fetch?: typeof fetch;
  readonly now?: Date;
}

export function reportAppleFmBackendCapability(
  input: ReportAppleFmBackendCapabilityInput,
): Effect.Effect<ProbeBackendCapabilityReport, never> {
  return makeAppleFmClient({
    explicitBaseUrl: input.trustedBackendBaseUrl,
    env: input.env,
    fetch: input.fetch,
    now: input.now,
  }).pipe(
    Effect.flatMap((client) =>
      client.health().pipe(
        Effect.map((readiness): ProbeBackendCapabilityReport => ({
          kind: "probe_backend_capability_report",
          runnerId: input.runner.runnerId,
          runnerKind: input.runner.kind,
          backendKind: APPLE_FM_BACKEND_KIND,
          profileId: client.profile.id,
          model: readiness.health?.modelId ?? readiness.health?.model ?? client.profile.model,
          capability: PROBE_APPLE_FM_BACKEND_CAPABILITY,
          advertisedCapabilities: readiness.ready ? [PROBE_APPLE_FM_BACKEND_CAPABILITY] : [],
          available: readiness.ready,
          status: readiness.status,
          baseUrl: redactUrl(client.profile.baseUrl),
          platform: readiness.health?.platform,
          version: readiness.health?.version,
          unavailableReason: readiness.unavailableReason,
          message: readiness.message,
          requirements: {
            appleSilicon: "required",
            appleIntelligence: "required",
            liveHealth: "required",
          },
          support: {
            snapshotStreaming: true,
            toolCallbacks: true,
          },
          receipt: readiness.receipt,
          observedAt: (input.now ?? new Date()).toISOString(),
          contentRedacted: true,
        })),
      ),
    ),
    Effect.catch(() =>
      Effect.succeed({
        kind: "probe_backend_capability_report" as const,
        runnerId: input.runner.runnerId,
        runnerKind: input.runner.kind,
        backendKind: APPLE_FM_BACKEND_KIND,
        profileId: "apple-fm-local",
        model: "apple-foundation-model",
        capability: PROBE_APPLE_FM_BACKEND_CAPABILITY,
        advertisedCapabilities: [],
        available: false,
        status: "malformed" as const,
        baseUrl: "[redacted-invalid-url]",
        unavailableReason: "malformed_response",
        message: "Apple FM backend capability profile could not be resolved",
        requirements: {
          appleSilicon: "required" as const,
          appleIntelligence: "required" as const,
          liveHealth: "required" as const,
        },
        support: {
          snapshotStreaming: true,
          toolCallbacks: true,
        },
        receipt: {
          kind: "probe_backend_availability",
          backendKind: APPLE_FM_BACKEND_KIND,
          profileId: "apple-fm-local",
          model: "apple-foundation-model",
          baseUrl: "[redacted-invalid-url]",
          ready: false,
          unavailableReason: "malformed_response",
          message: "Apple FM backend capability profile could not be resolved",
          observedAt: (input.now ?? new Date()).toISOString(),
          contentRedacted: true,
        },
        observedAt: (input.now ?? new Date()).toISOString(),
        contentRedacted: true as const,
      }),
    ),
  );
}

