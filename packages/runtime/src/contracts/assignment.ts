import { Effect, Schema as S } from "effect";
import { APPLE_FM_BACKEND_KIND } from "../backends/apple-fm/contract";
import {
  ChatGptCodexProvider,
  ProviderAccountRef,
  ProviderAuthGrantRef,
  validateProbePublicProjection,
  type ProbePublicProjectionUnsafe,
} from "./provider-account";

export const ProbeRepositoryRef = S.Struct({
  url: S.optional(S.String),
  path: S.optional(S.String),
  branch: S.optional(S.String),
  commit: S.optional(S.String),
});
export type ProbeRepositoryRef = typeof ProbeRepositoryRef.Type;

export const ProbeAssignmentBackend = S.Struct({
  kind: S.Literal(APPLE_FM_BACKEND_KIND),
  profile: S.optional(S.String),
});
export type ProbeAssignmentBackend = typeof ProbeAssignmentBackend.Type;

export const ProbeRunAssignment = S.Struct({
  assignmentId: S.String,
  runnerSessionId: S.String,
  goal: S.String,
  runtime: S.optional(S.String),
  backend: S.optional(ProbeAssignmentBackend),
  provider: S.optional(ChatGptCodexProvider),
  providerAccountRef: S.optional(ProviderAccountRef),
  authGrantRef: S.optional(ProviderAuthGrantRef),
  leaseRef: S.optional(S.String),
  repo: S.optional(ProbeRepositoryRef),
  callbackUrl: S.optional(S.String),
  sandbox: S.optional(S.Record(S.String, S.Unknown)),
});
export type ProbeRunAssignment = typeof ProbeRunAssignment.Type;

export class ProbeAssignmentParseError extends S.TaggedErrorClass<ProbeAssignmentParseError>()(
  "ProbeAssignmentParseError",
  {
    reason: S.String,
  },
) {}

export function decodeProbeRunAssignment(
  value: unknown,
): Effect.Effect<ProbeRunAssignment, ProbeAssignmentParseError | ProbePublicProjectionUnsafe> {
  return S.decodeUnknownEffect(ProbeRunAssignment)(value).pipe(
    Effect.mapError(
      (error) =>
        new ProbeAssignmentParseError({
          reason: String(error),
        }),
    ),
    Effect.tap((assignment) => validateProbePublicProjection(assignment, "assignment")),
  );
}

export function assignmentRequiresProviderGrant(assignment: ProbeRunAssignment): boolean {
  return assignment.provider === "chatgpt_codex" || assignment.providerAccountRef !== undefined || assignment.authGrantRef !== undefined;
}

export function assignmentSelectsAppleFmBackend(
  assignment: ProbeRunAssignment,
): assignment is ProbeRunAssignment & { readonly backend: ProbeAssignmentBackend } {
  return assignment.backend?.kind === APPLE_FM_BACKEND_KIND;
}

export function requireAppleFmAssignmentBackend(
  assignment: ProbeRunAssignment,
): Effect.Effect<ProbeAssignmentBackend, ProbeAssignmentParseError> {
  return assignmentSelectsAppleFmBackend(assignment)
    ? Effect.succeed(assignment.backend)
    : Effect.fail(new ProbeAssignmentParseError({ reason: "assignment is not selecting apple_fm_bridge" }));
}

export function requireAssignmentGrantRefs(
  assignment: ProbeRunAssignment,
): Effect.Effect<
  ProbeRunAssignment & { readonly providerAccountRef: ProviderAccountRef; readonly authGrantRef: ProviderAuthGrantRef },
  ProbeAssignmentParseError
> {
  if (assignment.providerAccountRef === undefined) {
    return Effect.fail(new ProbeAssignmentParseError({ reason: "assignment is missing providerAccountRef" }));
  }

  if (assignment.authGrantRef === undefined) {
    return Effect.fail(new ProbeAssignmentParseError({ reason: "assignment is missing authGrantRef" }));
  }

  return Effect.succeed(
    assignment as ProbeRunAssignment & {
      readonly providerAccountRef: ProviderAccountRef;
      readonly authGrantRef: ProviderAuthGrantRef;
    },
  );
}
