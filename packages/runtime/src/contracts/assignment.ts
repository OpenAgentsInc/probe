import { Effect, Schema as S } from "effect";
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

export const ProbeRunAssignment = S.Struct({
  assignmentId: S.String,
  runnerSessionId: S.String,
  goal: S.String,
  runtime: S.optional(S.String),
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
