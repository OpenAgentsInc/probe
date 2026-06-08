import { Effect, Schema as S } from "effect";
import { type ResolveProbeBackendProfileOptions, type ResolvedProbeBackendProfile } from "../backend-profile";
import { resolveGeminiBackendProfile, type ProbeBackendRegistryError } from "../registry";
import { makeGeminiAuthHeaders, resolveGeminiApiKey, type ResolvedGeminiApiKey } from "./auth";
import { geminiEndpointPath, lowerProbeLlmRequestToGeminiBody, parseGeminiSseStream } from "./protocol";
import {
  ProbeLlmEvents,
  makeProbeLlmMessage,
  makeProbeLlmRequest,
  makeProbeLlmToolResult,
  type ProbeLlmEvent,
  type ProbeLlmRequest,
} from "../../llm";
import { dispatchProbeLlmTool } from "../../llm/tool-runtime";
import { type ProbeLlmTools } from "../../llm/tool";

export interface GeminiClientOptions extends ResolveProbeBackendProfileOptions {
  readonly apiKey?: string;
  readonly env?: Readonly<Record<string, string | undefined>>;
  readonly fetch?: typeof fetch;
}

export interface GeminiClient {
  readonly profile: ResolvedProbeBackendProfile;
  readonly apiKey: {
    readonly source: ResolvedGeminiApiKey["source"];
    readonly redacted: true;
  };
  readonly complete: (input: GeminiCompleteInput) => Effect.Effect<GeminiCompleteResult, GeminiClientError>;
}

export interface GeminiCompleteInput {
  readonly request: ProbeLlmRequest;
  readonly tools?: ProbeLlmTools;
  readonly maxModelRoundTrips?: number;
}

export interface GeminiCompleteResult {
  readonly profile: ResolvedProbeBackendProfile;
  readonly events: ReadonlyArray<ProbeLlmEvent>;
  readonly text: string;
  readonly finalRequest: ProbeLlmRequest;
  readonly roundTrips: number;
}

export class GeminiClientError extends S.TaggedErrorClass<GeminiClientError>()("GeminiClientError", {
  reason: S.String,
  failureClass: S.String,
  statusCode: S.optional(S.Number),
}) {}

export function makeGeminiClient(
  options: GeminiClientOptions = {},
): Effect.Effect<GeminiClient, ProbeBackendRegistryError | GeminiClientError> {
  return Effect.gen(function* () {
    const profile = yield* resolveGeminiBackendProfile(options);
    const apiKey = yield* resolveGeminiApiKey({ apiKey: options.apiKey, env: options.env, profileId: profile.id }).pipe(
      Effect.mapError(
        (error) =>
          new GeminiClientError({
            reason: error.reason,
            failureClass: "missing_credential",
          }),
      ),
    );
    const fetchImpl = options.fetch ?? fetch;

    return {
      profile,
      apiKey: {
        source: apiKey.source,
        redacted: true as const,
      },
      complete: (input) => completeGemini({ profile, apiKey, fetchImpl, input }),
    };
  });
}

function completeGemini(input: {
  readonly profile: ResolvedProbeBackendProfile;
  readonly apiKey: ResolvedGeminiApiKey;
  readonly fetchImpl: typeof fetch;
  readonly input: GeminiCompleteInput;
}): Effect.Effect<GeminiCompleteResult, GeminiClientError> {
  return Effect.gen(function* () {
    const maxModelRoundTrips = input.input.maxModelRoundTrips ?? 8;
    let request = input.input.request;
    let events: ProbeLlmEvent[] = [];
    let roundTrips = 0;

    while (roundTrips < maxModelRoundTrips) {
      roundTrips += 1;
      const modelEvents = yield* callGemini(input.profile, input.apiKey, input.fetchImpl, request);
      events = [...events, ...modelEvents];
      const toolCalls = modelEvents.filter(ProbeLlmEvents.isToolCall);

      if (toolCalls.length === 0) {
        return {
          profile: input.profile,
          events,
          text: collectText(events),
          finalRequest: request,
          roundTrips,
        };
      }

      const toolResultParts = [];

      for (const call of toolCalls) {
        const dispatched = yield* dispatchProbeLlmTool(input.input.tools ?? {}, call);
        events = [...events, ...dispatched.events];
        toolResultParts.push(makeProbeLlmToolResult({ id: call.id, name: call.name, result: dispatched.result }));
      }

      request = makeProbeLlmRequest({
        ...request,
        messages: [
          ...request.messages,
          makeProbeLlmMessage(
            "assistant",
            toolCalls.map((call) => ({
              type: "tool-call" as const,
              id: call.id,
              name: call.name,
              input: call.input,
              providerMetadata: call.providerMetadata,
            })),
          ),
          makeProbeLlmMessage("tool", toolResultParts),
        ],
      });
    }

    return yield* Effect.fail(
      new GeminiClientError({
        reason: "Gemini tool-call round-trip limit reached",
        failureClass: "round_trip_limit",
      }),
    );
  });
}

function callGemini(
  profile: ResolvedProbeBackendProfile,
  apiKey: ResolvedGeminiApiKey,
  fetchImpl: typeof fetch,
  request: ProbeLlmRequest,
): Effect.Effect<ReadonlyArray<ProbeLlmEvent>, GeminiClientError> {
  return Effect.gen(function* () {
    const endpoint = new URL(geminiEndpointPath(request.model.model), withTrailingSlash(profile.baseUrl));
    const response = yield* Effect.tryPromise({
      try: () =>
        fetchImpl(endpoint, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            ...makeGeminiAuthHeaders(apiKey),
          },
          body: JSON.stringify(lowerProbeLlmRequestToGeminiBody(request)),
        }),
      catch: (error) =>
        new GeminiClientError({
          reason: `Gemini request failed: ${String(error)}`,
          failureClass: "request_failed",
        }),
    });
    const rawText = yield* Effect.tryPromise({
      try: () => response.text(),
      catch: (error) =>
        new GeminiClientError({
          reason: `Gemini response could not be read: ${String(error)}`,
          failureClass: "malformed_response",
        }),
    });

    if (!response.ok) {
      return yield* Effect.fail(
        new GeminiClientError({
          reason: `Gemini returned HTTP ${response.status}`,
          failureClass: `http_${response.status}`,
          statusCode: response.status,
        }),
      );
    }

    return yield* parseGeminiSseStream(rawText).pipe(
      Effect.mapError(
        (error) =>
          new GeminiClientError({
            reason: error.reason,
            failureClass: error.failureClass,
          }),
      ),
    );
  });
}

function collectText(events: ReadonlyArray<ProbeLlmEvent>): string {
  return events.flatMap((event) => (event.type === "text-delta" ? [event.text] : [])).join("");
}

function withTrailingSlash(value: string): string {
  return value.endsWith("/") ? value : `${value}/`;
}
