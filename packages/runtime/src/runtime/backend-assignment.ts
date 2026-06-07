import { Effect, Schema as S } from "effect";
import { AppleFmBackendError, makeAppleFmClient, type AppleFmPlainTextCompletion } from "../backends/apple-fm/client";
import { APPLE_FM_BACKEND_KIND } from "../backends/apple-fm/contract";
import { requireAppleFmAssignmentBackend, type ProbeRunAssignment } from "../contracts/assignment";
import { type AppleFmBackendAvailabilityReceipt, type AppleFmBackendFailureReceipt } from "../backends/apple-fm/receipts";
import {
  authorizeRunnerForAssignment,
  type ProbeRunnerAssignmentProof,
  type ProbeRunnerAuthorizationError,
  type ProbeRunnerIdentity,
} from "../runner/identity";
import { type ProbePublicProjectionUnsafe } from "../contracts/provider-account";
import { type ProbeBackendRegistryError } from "../backends/registry";

export const ProbeBackendRunEvent = S.Struct({
  kind: S.Literals(["probe_backend_run_started", "probe_backend_run_finished", "probe_backend_run_failed"]),
  assignmentId: S.String,
  runnerSessionId: S.String,
  backendKind: S.Literal(APPLE_FM_BACKEND_KIND),
  profileId: S.String,
  model: S.String,
  observedAt: S.String,
  contentRedacted: S.Literal(true),
  receipt: S.optional(S.Unknown),
});
export type ProbeBackendRunEvent = typeof ProbeBackendRunEvent.Type;

export interface ProbeBackendAssignmentRunInput {
  readonly runner: ProbeRunnerIdentity;
  readonly proof: ProbeRunnerAssignmentProof;
  readonly assignment: ProbeRunAssignment;
  readonly trustedBackendBaseUrl?: string;
  readonly env?: Readonly<Record<string, string | undefined>>;
  readonly fetch?: typeof fetch;
  readonly now?: Date;
}

export interface ProbeBackendAssignmentRunResult {
  readonly assignmentId: string;
  readonly runnerSessionId: string;
  readonly backendKind: typeof APPLE_FM_BACKEND_KIND;
  readonly profileId: string;
  readonly authRequired: false;
  readonly completion: AppleFmPlainTextCompletion;
  readonly events: ReadonlyArray<ProbeBackendRunEvent>;
}

export type ProbeBackendAssignmentRunError =
  | ProbeRunnerAuthorizationError
  | ProbePublicProjectionUnsafe
  | ProbeBackendRegistryError
  | ProbeBackendAssignmentError;

export class ProbeBackendAssignmentError extends S.TaggedErrorClass<ProbeBackendAssignmentError>()(
  "ProbeBackendAssignmentError",
  {
    reason: S.String,
    receipt: S.optional(S.Unknown),
    events: S.Array(ProbeBackendRunEvent),
  },
) {}

export function runProbeBackendAssignment(
  input: ProbeBackendAssignmentRunInput,
): Effect.Effect<ProbeBackendAssignmentRunResult, ProbeBackendAssignmentRunError> {
  return Effect.gen(function* () {
    yield* authorizeRunnerForAssignment(input.runner, input.proof, input.assignment, input.now);
    const backend = yield* requireAppleFmAssignmentBackend(input.assignment).pipe(
      Effect.mapError(
        (error) =>
          new ProbeBackendAssignmentError({
            reason: error.reason,
            events: [],
          }),
      ),
    );
    const client = yield* makeAppleFmClient({
      profileId: backend.profile,
      explicitBaseUrl: input.trustedBackendBaseUrl,
      env: input.env,
      fetch: input.fetch,
      now: input.now,
    });
    const observedAt = (input.now ?? new Date()).toISOString();
    const started = backendEvent({
      kind: "probe_backend_run_started",
      assignment: input.assignment,
      profileId: client.profile.id,
      model: client.profile.model,
      observedAt,
    });
    const readiness = yield* client.health();

    if (!readiness.ready) {
      const failed = backendEvent({
        kind: "probe_backend_run_failed",
        assignment: input.assignment,
        profileId: client.profile.id,
        model: client.profile.model,
        observedAt,
        receipt: readiness.receipt,
      });

      return yield* Effect.fail(
        new ProbeBackendAssignmentError({
          reason: readiness.message ?? `Apple FM backend is ${readiness.status}`,
          receipt: readiness.receipt,
          events: [started, failed],
        }),
      );
    }

    const completion = yield* client.completePlainText([{ role: "user", content: input.assignment.goal }]).pipe(
      Effect.mapError((error: AppleFmBackendError) => {
        const failed = backendEvent({
          kind: "probe_backend_run_failed",
          assignment: input.assignment,
          profileId: client.profile.id,
          model: client.profile.model,
          observedAt,
          receipt: error.receipt,
        });

        return new ProbeBackendAssignmentError({
          reason: error.reason,
          receipt: error.receipt,
          events: [started, failed],
        });
      }),
    );
    const finished = backendEvent({
      kind: "probe_backend_run_finished",
      assignment: input.assignment,
      profileId: client.profile.id,
      model: completion.response.model ?? client.profile.model,
      observedAt,
      receipt: completion.receipt,
    });

    return {
      assignmentId: input.assignment.assignmentId,
      runnerSessionId: input.assignment.runnerSessionId,
      backendKind: APPLE_FM_BACKEND_KIND,
      profileId: client.profile.id,
      authRequired: false,
      completion,
      events: [started, finished],
    };
  });
}

function backendEvent(input: {
  readonly kind: ProbeBackendRunEvent["kind"];
  readonly assignment: ProbeRunAssignment;
  readonly profileId: string;
  readonly model: string;
  readonly observedAt: string;
  readonly receipt?: AppleFmBackendAvailabilityReceipt | AppleFmBackendFailureReceipt | AppleFmPlainTextCompletion["receipt"];
}): ProbeBackendRunEvent {
  return {
    kind: input.kind,
    assignmentId: input.assignment.assignmentId,
    runnerSessionId: input.assignment.runnerSessionId,
    backendKind: APPLE_FM_BACKEND_KIND,
    profileId: input.profileId,
    model: input.model,
    observedAt: input.observedAt,
    contentRedacted: true,
    receipt: input.receipt,
  };
}

