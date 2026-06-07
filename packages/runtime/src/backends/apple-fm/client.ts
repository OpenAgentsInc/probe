import { Effect, Schema as S } from "effect";
import { type ResolvedProbeBackendProfile, type ResolveProbeBackendProfileOptions } from "../backend-profile";
import { resolveAppleFmBackendProfile, type ProbeBackendRegistryError } from "../registry";
import {
  AppleFmChatCompletionResponse,
  AppleFmHealthResponse,
  type AppleFmChatCompletionResponse,
  type AppleFmChatMessage,
  type AppleFmHealthResponse,
  type AppleFmUnavailableReason,
  type AppleFmUsageMeasurement,
} from "./contract";
import {
  AppleFmBackendFailureReceipt,
  makeAppleFmTranscriptReceipt,
  makeAppleFmAvailabilityReceipt,
  makeAppleFmFailureReceipt,
  type AppleFmBackendAvailabilityReceipt,
  type AppleFmBackendTranscriptReceipt,
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
  readonly completePlainText: (
    messages: ReadonlyArray<AppleFmChatMessage>,
  ) => Effect.Effect<AppleFmPlainTextCompletion, AppleFmBackendError>;
  readonly smoke: (prompt: string) => Effect.Effect<AppleFmPlainTextCompletion, AppleFmBackendError>;
}

export interface AppleFmPlainTextCompletion {
  readonly profile: ResolvedProbeBackendProfile;
  readonly text: string;
  readonly response: AppleFmChatCompletionResponse;
  readonly usage: AppleFmUsageMeasurement;
  readonly receipt: AppleFmBackendTranscriptReceipt;
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
      completePlainText: (messages) => completeAppleFmPlainText(profile, messages, fetchImpl, now()),
      smoke: (prompt) =>
        client.requireReady().pipe(
          Effect.flatMap(() =>
            completeAppleFmPlainText(
              profile,
              [
                {
                  role: "user",
                  content: prompt,
                },
              ],
              fetchImpl,
              now(),
            ),
          ),
        ),
    };

    return client;
  });
}

export function completeAppleFmPlainText(
  profile: ResolvedProbeBackendProfile,
  messages: ReadonlyArray<AppleFmChatMessage>,
  fetchImpl: typeof fetch = fetch,
  observedAt = new Date().toISOString(),
): Effect.Effect<AppleFmPlainTextCompletion, AppleFmBackendError> {
  return Effect.gen(function* () {
    const endpoint = new URL("/v1/chat/completions", withTrailingSlash(profile.baseUrl));
    const response = yield* Effect.tryPromise({
      try: () =>
        fetchImpl(endpoint, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            model: profile.model,
            messages,
          }),
        }),
      catch: (error) =>
        new AppleFmBackendError({
          reason: `Apple FM completion request failed: ${String(error)}`,
          failureClass: "bridge_unreachable",
          receipt: makeAppleFmFailureReceipt({
            profileId: profile.id,
            model: profile.model,
            baseUrl: profile.baseUrl,
            failureClass: "bridge_unreachable",
            message: `Apple FM completion request failed: ${String(error)}`,
            observedAt,
          }),
        }),
    });

    const raw = yield* Effect.tryPromise({
      try: () => response.json(),
      catch: (error) =>
        new AppleFmBackendError({
          reason: `Apple FM completion response was not JSON: ${String(error)}`,
          failureClass: "malformed_response",
          receipt: makeAppleFmFailureReceipt({
            profileId: profile.id,
            model: profile.model,
            baseUrl: profile.baseUrl,
            failureClass: "malformed_response",
            message: `Apple FM completion response was not JSON: ${String(error)}`,
            observedAt,
          }),
        }),
    });

    if (!response.ok) {
      const errorMessage = bridgeErrorMessage(raw) ?? `Apple FM completion returned HTTP ${response.status}`;
      return yield* Effect.fail(
        new AppleFmBackendError({
          reason: errorMessage,
          failureClass: `completion_http_${response.status}`,
          receipt: makeAppleFmFailureReceipt({
            profileId: profile.id,
            model: profile.model,
            baseUrl: profile.baseUrl,
            failureClass: `completion_http_${response.status}`,
            message: errorMessage,
            observedAt,
          }),
        }),
      );
    }

    const normalized = normalizeChatCompletion(raw, profile.model);
    const decoded = yield* S.decodeUnknownEffect(AppleFmChatCompletionResponse)(normalized).pipe(
      Effect.mapError(
        (error) =>
          new AppleFmBackendError({
            reason: `Apple FM completion response was malformed: ${String(error)}`,
            failureClass: "malformed_response",
            receipt: makeAppleFmFailureReceipt({
              profileId: profile.id,
              model: profile.model,
              baseUrl: profile.baseUrl,
              failureClass: "malformed_response",
              message: `Apple FM completion response was malformed: ${String(error)}`,
              observedAt,
            }),
          }),
      ),
    );

    const choice = decoded.choices[0];

    if (choice === undefined || choice.message.content.length === 0) {
      return yield* Effect.fail(
        new AppleFmBackendError({
          reason: "Apple FM completion response did not include assistant text",
          failureClass: "empty_completion",
          receipt: makeAppleFmFailureReceipt({
            profileId: profile.id,
            model: profile.model,
            baseUrl: profile.baseUrl,
            failureClass: "empty_completion",
            message: "Apple FM completion response did not include assistant text",
            observedAt,
          }),
        }),
      );
    }

    const usage = decoded.usage ?? { truth: "unknown" as const };

    return {
      profile,
      text: choice.message.content,
      response: decoded,
      usage,
      receipt: makeAppleFmTranscriptReceipt({
        profileId: profile.id,
        model: decoded.model ?? profile.model,
        usage,
        observedAt,
      }),
    };
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

function normalizeChatCompletion(value: unknown, fallbackModel: string): unknown {
  if (typeof value !== "object" || value === null) {
    return value;
  }

  const input = value as Record<string, unknown>;
  const choices = Array.isArray(input.choices) ? input.choices.map(normalizeChoice) : [];

  return {
    id: typeof input.id === "string" ? input.id : undefined,
    model: typeof input.model === "string" ? input.model : fallbackModel,
    choices,
    usage: normalizeUsage(input.usage),
  };
}

function normalizeChoice(value: unknown): unknown {
  if (typeof value !== "object" || value === null) {
    return value;
  }

  const input = value as Record<string, unknown>;
  const message = typeof input.message === "object" && input.message !== null ? input.message as Record<string, unknown> : {};

  return {
    index: typeof input.index === "number" ? input.index : undefined,
    message: {
      role: normalizeRole(message.role),
      content: typeof message.content === "string" ? message.content : "",
      name: typeof message.name === "string" ? message.name : undefined,
      toolCallId: typeof message.toolCallId === "string" ? message.toolCallId : undefined,
    },
    finishReason: normalizeFinishReason(input.finishReason ?? input.finish_reason),
  };
}

function normalizeUsage(value: unknown): AppleFmUsageMeasurement {
  if (typeof value !== "object" || value === null) {
    return {
      truth: "unknown",
    };
  }

  const input = value as Record<string, unknown>;
  const promptTokens = numberField(input.promptTokens) ?? numberField(input.prompt_tokens);
  const completionTokens = numberField(input.completionTokens) ?? numberField(input.completion_tokens);
  const totalTokens = numberField(input.totalTokens) ?? numberField(input.total_tokens);
  const hasTokenCounts = promptTokens !== undefined || completionTokens !== undefined || totalTokens !== undefined;
  const truth = input.truth === "exact" || input.truth === "estimated" || input.truth === "unknown"
    ? input.truth
    : hasTokenCounts
      ? "estimated"
      : "unknown";

  return {
    truth,
    promptTokens,
    completionTokens,
    totalTokens,
  };
}

function numberField(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function normalizeRole(value: unknown): AppleFmChatMessage["role"] {
  return value === "system" || value === "user" || value === "assistant" || value === "tool" ? value : "assistant";
}

function normalizeFinishReason(value: unknown): AppleFmChatCompletionResponse["choices"][number]["finishReason"] {
  if (
    value === "stop" ||
    value === "length" ||
    value === "tool_calls" ||
    value === "content_filter" ||
    value === "error" ||
    value === "unknown"
  ) {
    return value;
  }

  return "unknown";
}

function bridgeErrorMessage(value: unknown): string | undefined {
  if (typeof value !== "object" || value === null) {
    return undefined;
  }

  const input = value as Record<string, unknown>;
  const error = input.error;

  if (typeof error === "string") {
    return error;
  }

  if (typeof error === "object" && error !== null) {
    const errorObject = error as Record<string, unknown>;

    if (typeof errorObject.message === "string") {
      return errorObject.message;
    }
  }

  if (typeof input.message === "string") {
    return input.message;
  }

  return undefined;
}
