#!/usr/bin/env bun
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, relative, resolve, sep } from "node:path";
import { createInterface } from "node:readline/promises";
import { Effect, Schema as S } from "effect";
import {
  AppleFmBackendError,
  makeAppleFmClient,
  type AppleFmPlainTextCompletion,
  type AppleFmReadiness,
  type AppleFmToolStreamResult,
} from "./backends/apple-fm/client";
import {
  type AppleFmBlueprintToolProjection,
  projectProbeToolMenuToAppleFm,
} from "./backends/apple-fm/blueprint-tools";
import { makeAppleFmToolStreamProgramRunEvidence } from "./backends/apple-fm/program-run-evidence";
import { makeAppleFmToolCallbackSession } from "./backends/apple-fm/tools";
import { GeminiClientError, makeGeminiClient, type GeminiCompleteResult } from "./backends/gemini/client";
import { GEMINI_API_PROFILE_ID, GEMINI_DEFAULT_MODEL_ID } from "./backends/gemini/contract";
import {
  loadBlueprintSignatureRegistry,
  lookupBlueprintSignatures,
  planProbeToolMenu,
} from "./blueprint";
import {
  defineProbeLlmTool,
  makeProbeLlmMessage,
  makeProbeLlmRequest,
  probeLlmToolDefinitions,
  type ProbeLlmMessage,
  type ProbeLlmRequest,
  type ProbeLlmTools,
} from "./llm";
import { PROBE_APPLE_FM_BACKEND_CAPABILITY } from "./runner/identity";
import { makeOmegaAccountClient, type OmegaAccountClient } from "./omega/account-client";
import { sanitizeProbePublicProjection } from "./contracts/provider-account";
import { type ProbeRunnerIdentity } from "./runner/identity";

export interface ProbeCliResult {
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
}

export interface ProbeCliDeps {
  readonly accountClient?: OmegaAccountClient;
  readonly env?: Readonly<Record<string, string | undefined>>;
  readonly fetch?: typeof fetch;
  readonly now?: Date;
}

export class ProbeCliError extends S.TaggedErrorClass<ProbeCliError>()("ProbeCliError", {
  message: S.String,
}) {}

export function runProbeCli(argv: ReadonlyArray<string>, deps: ProbeCliDeps = {}): Effect.Effect<ProbeCliResult, never> {
  return handleProbeCli(argv, deps).pipe(
    Effect.catch((error: ProbeCliError) =>
      Effect.succeed({
        exitCode: 1,
        stdout: "",
        stderr: `${error.message}\n`,
      }),
    ),
  );
}

function handleProbeCli(
  argv: ReadonlyArray<string>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const [namespace, command, ...rest] = argv;
    const options = parseOptions(rest);

    if (namespace === "omega" && command === "link") {
      return yield* linkOmega(options, deps);
    }

    if (namespace === "auth" && command === "accounts") {
      return yield* listAccounts(options, deps);
    }

    if (namespace === "auth" && command === "add" && rest[0] === "chatgpt") {
      return yield* addChatGptAccount(parseOptions(rest.slice(1)), deps);
    }

    if (namespace === "backend" && command === "gemini" && rest[0] === "smoke") {
      return yield* geminiSmoke(parseOptions(rest.slice(1)), deps);
    }

    if (namespace === "backend" && command === "gemini" && rest[0] === "complete") {
      return yield* geminiComplete(parseOptions(rest.slice(1)), deps);
    }

    if (namespace === "chat") {
      return yield* geminiChatOnce(parseOptions(argv.slice(1)), deps);
    }

    if (namespace === "apple-fm" && command === "status") {
      return yield* appleFmStatus(options, deps);
    }

    if (namespace === "apple-fm" && command === "smoke") {
      return yield* appleFmSmoke(options, deps);
    }

    if (namespace === "apple-fm" && command === "tool-stream-demo") {
      return yield* appleFmToolStreamDemo(options, deps);
    }

    return {
      exitCode: 1,
      stdout: usage(),
      stderr: "",
    };
  });
}

function geminiSmoke(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return geminiCompletionCommand({
    title: "Gemini smoke",
    options,
    deps,
    defaultPrompt: "Reply with: probe gemini smoke ok.",
  });
}

function geminiComplete(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return geminiCompletionCommand({
    title: "Gemini completion",
    options,
    deps,
    defaultPrompt: "Reply with a concise Probe Gemini completion.",
  });
}

function geminiChatOnce(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const prompt = stringOption(options, "prompt");

    if (prompt === undefined) {
      return {
        exitCode: 1,
        stdout: [
          "Probe Gemini chat is interactive.",
          "Run `probe chat` for the prompt, or `probe chat --prompt TEXT` for one turn.",
        ].join("\n") + "\n",
        stderr: "",
      };
    }

    const model = stringOption(options, "model") ?? GEMINI_DEFAULT_MODEL_ID;
    const client = yield* makeGeminiClient({
      profileId: stringOption(options, "profile") ?? deps.env?.PROBE_BACKEND_PROFILE ?? GEMINI_API_PROFILE_ID,
      explicitBaseUrl: stringOption(options, "base-url"),
      env: deps.env,
      fetch: deps.fetch,
      now: deps.now,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: "reason" in error ? error.reason : String(error) })));
    const tools = makeGeminiChatTools();
    const request = makeGeminiChatRequest({
      messages: [],
      model,
      prompt,
      maxTokens: numberOption(options, "max-tokens") ?? 1024,
      tools,
    });
    const result = yield* client.complete({ request, tools, maxModelRoundTrips: 8 }).pipe(
      Effect.catch((error: GeminiClientError) => Effect.succeed(error)),
    );

    if (result instanceof GeminiClientError) {
      return {
        exitCode: 1,
        stdout: formatGeminiFailure("Probe Gemini chat", result),
        stderr: "",
      };
    }

    return {
      exitCode: 0,
      stdout: formatGeminiChatTurn({
        apiKeySource: client.apiKey.source,
        includeHeader: true,
        result,
      }),
      stderr: "",
    };
  });
}

function geminiCompletionCommand(input: {
  readonly title: string;
  readonly options: Record<string, string | true>;
  readonly deps: ProbeCliDeps;
  readonly defaultPrompt: string;
}): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const model = stringOption(input.options, "model") ?? GEMINI_DEFAULT_MODEL_ID;
    const prompt = stringOption(input.options, "prompt") ?? input.defaultPrompt;
    const client = yield* makeGeminiClient({
      profileId: stringOption(input.options, "profile") ?? input.deps.env?.PROBE_BACKEND_PROFILE ?? GEMINI_API_PROFILE_ID,
      explicitBaseUrl: stringOption(input.options, "base-url"),
      env: input.deps.env,
      fetch: input.deps.fetch,
      now: input.deps.now,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: "reason" in error ? error.reason : String(error) })));
    const result = yield* client.complete({
      request: makeProbeLlmRequest({
        model: { provider: "google", model },
        prompt,
        generation: { maxTokens: numberOption(input.options, "max-tokens") ?? 256, temperature: 0 },
      }),
    }).pipe(Effect.catch((error: GeminiClientError) => Effect.succeed(error)));

    if (result instanceof GeminiClientError) {
      return {
        exitCode: 1,
        stdout: formatGeminiFailure(input.title, result),
        stderr: "",
      };
    }

    return {
      exitCode: 0,
      stdout: formatGeminiCompletion(input.title, client.apiKey.source, result),
      stderr: "",
    };
  });
}

function appleFmToolStreamDemo(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const client = yield* makeAppleFmClient({
      profileId: stringOption(options, "profile"),
      explicitBaseUrl: stringOption(options, "base-url"),
      env: deps.env,
      fetch: deps.fetch,
      now: deps.now,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: error.reason })));
    yield* client.requireReady().pipe(
      Effect.mapError((error) => new ProbeCliError({ message: `${error.failureClass}: ${error.reason}` })),
    );
    const requestedPath = stringOption(options, "path") ?? "README.md";
    const prompt =
      stringOption(options, "prompt") ??
      `Use the read_file tool to read ${requestedPath}, then stream one concise sentence naming the file and its first heading.`;
    const registryView = yield* loadBlueprintSignatureRegistry({ sourceKind: "staticFixture" }).pipe(
      Effect.mapError((error) => new ProbeCliError({ message: error.reason })),
    );
    const lookup = yield* lookupBlueprintSignatures({
      backendCapabilityRefs: [PROBE_APPLE_FM_BACKEND_CAPABILITY, "probe.blueprint.tool_menu"],
      lookupId: "blueprint_signature_lookup.apple_fm.tool_stream_demo",
      registryView,
      request: {
        actorRef: "actor.probe.cli",
        allowedSurfaces: ["agent_api"],
        backendKind: "apple_fm_bridge",
        contextPackRef: `context_pack.probe.cli.${requestedPath}`,
        programSignatureIds: ["program_signature.probe.tool_menu.project.v1"],
        riskCeiling: "medium",
      },
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: error.reason })));
    const menu = yield* planProbeToolMenu({
      backendKind: "apple_fm_bridge",
      contextPackRefs: [lookup.contextPackRef ?? `context_pack.probe.cli.${requestedPath}`],
      deniedToolRefs: [],
      lookup,
      maxToolCount: 1,
      menuId: "probe_tool_menu.apple_fm.tool_stream_demo",
      sourceAuthorityRefs: [`source_authority.probe.workspace.${requestedPath}`],
      supportedToolRefs: ["tool.probe.read_file"],
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: error.reason })));
    const projectedMenu = yield* projectProbeToolMenuToAppleFm({
      enumHints: {
        "tool.probe.read_file": {
          path: [requestedPath],
        },
      },
      executors: {
        "tool.probe.read_file": (input) => readWorkspaceFile(input, requestedPath),
      },
      menu,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: error.reason })));
    const toolSession = makeAppleFmToolCallbackSession({
      tools: projectedMenu.toolDefinitions,
      maxModelRoundTrips: 4,
      now: deps.now,
    });
    const result = yield* client.streamSessionWithTools({
      prompt,
      instructions: "Use available tools when the user asks to inspect a local file. Keep the final answer concise.",
      toolSession,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: `${error.failureClass}: ${error.reason}` })));
    const programRunEvidence = yield* makeAppleFmToolStreamProgramRunEvidence({
      actorRef: "actor.probe.cli",
      menu,
      observedAt: (deps.now ?? new Date()).toISOString(),
      promptSummaryRef: `prompt_summary.probe.cli.${requestedPath}`,
      projection: projectedMenu.projection,
      result,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: error.reason })));

    return {
      exitCode: 0,
      stdout: formatAppleFmToolStreamDemo(result, projectedMenu.projection, programRunEvidence),
      stderr: "",
    };
  });
}

function appleFmSmoke(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const client = yield* makeAppleFmClient({
      profileId: stringOption(options, "profile"),
      explicitBaseUrl: stringOption(options, "base-url"),
      env: deps.env,
      fetch: deps.fetch,
      now: deps.now,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: error.reason })));
    const prompt = stringOption(options, "prompt") ?? "Reply with: probe apple fm smoke ok.";
    const result = yield* client.smoke(prompt).pipe(
      Effect.catch((error: AppleFmBackendError) => Effect.succeed(error)),
    );

    if (result instanceof AppleFmBackendError) {
      return {
        exitCode: 1,
        stdout: formatAppleFmSmokeFailure(result),
        stderr: "",
      };
    }

    return {
      exitCode: 0,
      stdout: formatAppleFmSmokeCompletion(result),
      stderr: "",
    };
  });
}

function appleFmStatus(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const client = yield* makeAppleFmClient({
      profileId: stringOption(options, "profile"),
      explicitBaseUrl: stringOption(options, "base-url"),
      env: deps.env,
      fetch: deps.fetch,
      now: deps.now,
    }).pipe(Effect.mapError((error) => new ProbeCliError({ message: error.reason })));
    const readiness = yield* client.health();

    return {
      exitCode: readiness.ready ? 0 : 1,
      stdout: formatAppleFmStatus(readiness),
      stderr: "",
    };
  });
}

function linkOmega(options: Record<string, string | true>, deps: ProbeCliDeps): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const now = deps.now ?? new Date();
    const statePath = resolveStatePath(options, deps.env);
    const runner: ProbeRunnerIdentity = {
      runnerId: stringOption(options, "runner-id") ?? "probe-local",
      kind: runnerKindOption(options, "kind") ?? "local",
      linkedSubject: stringOption(options, "subject") ?? "local-user",
      linkedAt: now.toISOString(),
      capabilities: ["probe.run", "omega.grant.resolve"],
    };

    const state = sanitizeProbePublicProjection({
      version: 1,
      omegaBaseUrl: omegaBaseUrl(options, deps.env),
      runner,
    });

    yield* Effect.tryPromise({
      try: () => mkdir(dirname(statePath), { recursive: true }),
      catch: (error) => new ProbeCliError({ message: `failed to create Probe state directory: ${String(error)}` }),
    });

    yield* Effect.tryPromise({
      try: () => writeFile(statePath, `${JSON.stringify(state, null, 2)}\n`, { mode: 0o600 }),
      catch: (error) => new ProbeCliError({ message: `failed to write Probe Omega link state: ${String(error)}` }),
    });

    return {
      exitCode: 0,
      stdout: `Linked Probe runner ${runner.runnerId} to ${state.omegaBaseUrl}\nState: ${statePath}\n`,
      stderr: "",
    };
  });
}

function listAccounts(options: Record<string, string | true>, deps: ProbeCliDeps): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const client = deps.accountClient ?? makeOmegaAccountClient(clientOptions(options, deps.env));
    const response = yield* client.listProviderAccounts().pipe(
      Effect.mapError((error) => new ProbeCliError({ message: error.reason })),
    );

    if (response.accounts.length === 0) {
      return {
        exitCode: 0,
        stdout: "No Omega-connected ChatGPT accounts.\n",
        stderr: "",
      };
    }

    const lines = response.accounts.map((account) => {
      const label = account.accountLabel ?? account.providerAccountRef;
      const plan = account.planType === undefined ? "unknown-plan" : account.planType;
      return `${account.providerAccountRef}\t${label}\t${account.status}/${account.health}\t${plan}`;
    });

    return {
      exitCode: 0,
      stdout: `${lines.join("\n")}\n`,
      stderr: "",
    };
  });
}

function addChatGptAccount(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
): Effect.Effect<ProbeCliResult, ProbeCliError> {
  return Effect.gen(function* () {
    const client = deps.accountClient ?? makeOmegaAccountClient(clientOptions(options, deps.env));
    const started = yield* client.startChatGptDeviceLogin({ createNew: true }).pipe(
      Effect.mapError((error) => new ProbeCliError({ message: error.reason })),
    );
    const attempt = yield* client.readChatGptDeviceLogin(started.attemptId).pipe(
      Effect.mapError((error) => new ProbeCliError({ message: error.reason })),
    );

    return {
      exitCode: 0,
      stdout: [
        `Open ${started.verificationUrl}`,
        `Code ${started.userCode}`,
        `Attempt ${started.attemptId}: ${attempt.status}`,
        `Provider account ${attempt.providerAccountRef}`,
      ].join("\n") + "\n",
      stderr: "",
    };
  });
}

function parseOptions(args: ReadonlyArray<string>): Record<string, string | true> {
  const parsed: Record<string, string | true> = {};

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];

    if (!arg.startsWith("--")) {
      continue;
    }

    const key = arg.slice(2);
    const next = args[index + 1];

    if (next === undefined || next.startsWith("--")) {
      parsed[key] = true;
      continue;
    }

    parsed[key] = next;
    index += 1;
  }

  return parsed;
}

function clientOptions(options: Record<string, string | true>, env: Readonly<Record<string, string | undefined>> = {}) {
  return {
    baseUrl: omegaBaseUrl(options, env),
    bearerToken: stringOption(options, "token") ?? env.PROBE_OMEGA_BEARER_TOKEN,
  };
}

function omegaBaseUrl(options: Record<string, string | true>, env: Readonly<Record<string, string | undefined>> = {}) {
  return stringOption(options, "base-url") ?? env.PROBE_OMEGA_BASE_URL ?? "https://openagents.com";
}

function resolveStatePath(
  options: Record<string, string | true>,
  env: Readonly<Record<string, string | undefined>> = {},
): string {
  return resolve(stringOption(options, "state") ?? env.PROBE_STATE_PATH ?? ".probe/omega-link.json");
}

function stringOption(options: Record<string, string | true>, key: string): string | undefined {
  const value = options[key];
  return typeof value === "string" ? value : undefined;
}

function numberOption(options: Record<string, string | true>, key: string): number | undefined {
  const value = stringOption(options, key);

  if (value === undefined) {
    return undefined;
  }

  const parsed = Number(value);

  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

function runnerKindOption(options: Record<string, string | true>, key: string): ProbeRunnerIdentity["kind"] | undefined {
  const value = stringOption(options, key);

  return value === "local" || value === "shc" || value === "pylon" || value === "sandbox" ? value : undefined;
}

function usage(): string {
  return [
    "Usage:",
    "  probe omega link [--base-url URL] [--runner-id ID] [--subject USER_OR_TEAM] [--kind local|shc|pylon|sandbox]",
    "  probe auth accounts [--base-url URL]",
    "  probe auth add chatgpt [--base-url URL]",
    "  probe chat [--profile gemini-api] [--model gemini-2.5-flash] [--prompt TEXT]",
    "  probe backend gemini smoke [--profile gemini-api] [--model gemini-2.5-flash] [--prompt TEXT]",
    "  probe backend gemini complete [--profile gemini-api] [--model gemini-2.5-flash] [--prompt TEXT]",
    "  probe apple-fm status [--base-url URL] [--profile apple-fm-local]",
    "  probe apple-fm smoke [--base-url URL] [--profile apple-fm-local] [--prompt TEXT]",
    "  probe apple-fm tool-stream-demo [--base-url URL] [--path FILE] [--prompt TEXT]",
  ].join("\n") + "\n";
}

function formatAppleFmStatus(readiness: AppleFmReadiness): string {
  const health = readiness.health;
  const lines = [
    "Apple FM backend status",
    `profile: ${readiness.profile.id}`,
    `kind: ${readiness.profile.kind}`,
    `baseUrl: ${readiness.profile.baseUrl}`,
    `model: ${health?.modelId ?? health?.model ?? readiness.profile.model}`,
    `status: ${readiness.status}`,
  ];

  if (readiness.unavailableReason !== undefined) {
    lines.push(`unavailableReason: ${readiness.unavailableReason}`);
  }

  if (readiness.message !== undefined) {
    lines.push(`message: ${readiness.message}`);
  }

  if (health?.platform !== undefined) {
    lines.push(`platform: ${health.platform}`);
  }

  if (health?.version !== undefined) {
    lines.push(`version: ${health.version}`);
  }

  lines.push(`receipt: ${JSON.stringify(readiness.receipt)}`);

  return `${lines.join("\n")}\n`;
}

function formatAppleFmSmokeCompletion(completion: AppleFmPlainTextCompletion): string {
  return [
    "Apple FM smoke",
    `profile: ${completion.profile.id}`,
    `kind: ${completion.profile.kind}`,
    `model: ${completion.response.model ?? completion.profile.model}`,
    `assistant: ${completion.text}`,
    `usage: ${formatUsage(completion.usage)}`,
    `receipt: ${JSON.stringify(completion.receipt)}`,
  ].join("\n") + "\n";
}

function formatAppleFmSmokeFailure(error: AppleFmBackendError): string {
  const lines = [
    "Apple FM smoke failed",
    `failureClass: ${error.failureClass}`,
    `message: ${error.reason}`,
  ];

  if (error.receipt !== undefined) {
    lines.push(`receipt: ${JSON.stringify(error.receipt)}`);
  }

  return `${lines.join("\n")}\n`;
}

function formatGeminiCompletion(title: string, apiKeySource: string, completion: GeminiCompleteResult): string {
  return [
    title,
    `profile: ${completion.profile.id}`,
    `kind: ${completion.profile.kind}`,
    `model: ${completion.finalRequest.model.model}`,
    `apiKeySource: ${apiKeySource}`,
    "apiKeyRedacted: true",
    `assistant: ${completion.text}`,
    `roundTrips: ${completion.roundTrips}`,
    `usage: ${formatGeminiUsage(completion.receipt.usage)}`,
    `receipt: ${JSON.stringify(completion.receipt)}`,
  ].join("\n") + "\n";
}

function formatGeminiChatTurn(input: {
  readonly apiKeySource: string;
  readonly includeHeader?: boolean;
  readonly result: GeminiCompleteResult;
}): string {
  const lines: string[] = [];

  if (input.includeHeader === true) {
    lines.push("Probe Gemini chat");
    lines.push(`profile: ${input.result.profile.id}`);
    lines.push(`kind: ${input.result.profile.kind}`);
    lines.push(`model: ${input.result.finalRequest.model.model}`);
    lines.push(`apiKeySource: ${input.apiKeySource}`);
    lines.push("apiKeyRedacted: true");
  }

  for (const event of input.result.events) {
    if (event.type === "tool-call") {
      lines.push(`tool_call: ${event.name} ${safeJson(event.input)}`);
    }

    if (event.type === "tool-result") {
      lines.push(`tool_result: ${event.name} ${formatToolResultValue(event.result)}`);
    }

    if (event.type === "tool-error") {
      lines.push(`tool_error: ${event.name} ${event.message}`);
    }
  }

  lines.push(`assistant: ${input.result.text}`);
  lines.push(`roundTrips: ${input.result.roundTrips}`);
  lines.push(`usage: ${formatGeminiUsage(input.result.receipt.usage)}`);

  return `${lines.join("\n")}\n`;
}

function formatGeminiFailure(title: string, error: GeminiClientError): string {
  const lines = [
    `${title} failed`,
    `failureClass: ${error.failureClass}`,
    `message: ${error.reason}`,
  ];

  if (error.receipt !== undefined) {
    lines.push(`receipt: ${JSON.stringify(error.receipt)}`);
  }

  return `${lines.join("\n")}\n`;
}

function formatGeminiUsage(usage: GeminiCompleteResult["receipt"]["usage"]): string {
  if (usage === undefined) {
    return "unreported";
  }

  const parts: string[] = [];

  if (usage.inputTokens !== undefined) {
    parts.push(`input=${usage.inputTokens}`);
  }

  if (usage.outputTokens !== undefined) {
    parts.push(`output=${usage.outputTokens}`);
  }

  if (usage.totalTokens !== undefined) {
    parts.push(`total=${usage.totalTokens}`);
  }

  return parts.length === 0 ? "unreported" : parts.join(" ");
}

function formatUsage(usage: AppleFmPlainTextCompletion["usage"]): string {
  const parts = [`truth=${usage.truth}`];

  if (usage.promptTokens !== undefined) {
    parts.push(`prompt=${usage.promptTokens}`);
  }

  if (usage.completionTokens !== undefined) {
    parts.push(`completion=${usage.completionTokens}`);
  }

  if (usage.totalTokens !== undefined) {
    parts.push(`total=${usage.totalTokens}`);
  }

  return parts.join(" ");
}

function formatAppleFmToolStreamDemo(
  result: AppleFmToolStreamResult,
  projection?: AppleFmBlueprintToolProjection,
  programRunEvidence?: { readonly programRunRef: string; readonly inputSnapshotHash: string },
): string {
  const lines = [
    "Apple FM tool stream demo",
    `bridgeSessionId: ${result.bridgeSessionId}`,
    `events: ${result.events.map((event) => event.kind).join(" -> ")}`,
  ];

  if (projection !== undefined) {
    lines.push(`blueprintLookupId: ${projection.lookupId}`);
    lines.push(`blueprintMenuId: ${projection.menuId}`);
    lines.push(`blueprintRegistryVersionRef: ${projection.registryVersionRef}`);
    lines.push(`blueprintProgramSignatures: ${projection.programSignatureIds.join(",")}`);
    lines.push(`blueprintTools: ${projection.toolRefs.map((tool) => `${tool.toolRef}:${tool.toolName}`).join(",")}`);
  }

  if (programRunEvidence !== undefined) {
    lines.push(`programRunRef: ${programRunEvidence.programRunRef}`);
    lines.push(`programRunInputSnapshotHash: ${programRunEvidence.inputSnapshotHash}`);
  }

  for (const event of result.events) {
    if (event.kind === "assistant_snapshot" && event.content !== undefined) {
      lines.push(`snapshot: ${event.content}`);
    }
  }

  for (const entry of result.toolTranscript) {
    lines.push(`tool: ${entry.toolName} ${entry.status} ${JSON.stringify(entry.input)}`);
  }

  lines.push(`final: ${result.completion.text}`);
  lines.push(`usage: ${formatUsage(result.completion.usage)}`);
  lines.push(`receipt: ${JSON.stringify(result.completion.receipt)}`);

  return `${lines.join("\n")}\n`;
}

function readWorkspaceFile(
  input: Readonly<Record<string, unknown>>,
  allowedPath: string,
): Effect.Effect<{ readonly path: string; readonly content?: string; readonly error?: string }, never> {
  return Effect.gen(function* () {
    const path = typeof input.path === "string" ? input.path : allowedPath;
    const workspace = resolveProbeWorkspaceRoot();
    const absolutePath = resolve(workspace, path);
    const relativePath = relative(workspace, absolutePath);

    if (
      path !== allowedPath ||
      relativePath.startsWith("..") ||
      relativePath === "" ||
      relativePath.split(sep).includes("..")
    ) {
      return {
        path,
        error: "path is outside the Blueprint-selected file scope",
      };
    }

    const content = yield* Effect.tryPromise({
      try: () => readFile(absolutePath, "utf8"),
      catch: (error) => error,
    }).pipe(
      Effect.catch((error) =>
        Effect.succeed(`failed to read ${path}: ${String(error)}`),
      ),
    );

    return {
      path,
      content: typeof content === "string" ? content.slice(0, 4000) : String(content),
    };
  });
}

function makeGeminiChatTools(): ProbeLlmTools {
  return {
    read_file: defineProbeLlmTool({
      name: "read_file",
      description: "Read a UTF-8 text file under the current Probe workspace. Use this when the user asks about local code.",
      inputSchema: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "A relative file path under the workspace.",
          },
        },
        required: ["path"],
      },
      execute: readAnyWorkspaceFile,
    }),
    current_time: defineProbeLlmTool({
      name: "current_time",
      description: "Return the current local timestamp.",
      inputSchema: {
        type: "object",
        properties: {},
      },
      execute: () => Effect.succeed({ now: new Date().toISOString() }),
    }),
  };
}

function makeGeminiChatRequest(input: {
  readonly messages: ReadonlyArray<ProbeLlmMessage>;
  readonly model: string;
  readonly prompt: string;
  readonly maxTokens: number;
  readonly tools: ProbeLlmTools;
}): ProbeLlmRequest {
  return makeProbeLlmRequest({
    model: { provider: "google", model: input.model },
    system:
      "You are Probe, a concise coding agent. Use tools when they help inspect local code. " +
      "When you use a tool, continue to a direct final answer after the tool result.",
    messages: [...input.messages, makeProbeLlmMessage("user", input.prompt)],
    tools: probeLlmToolDefinitions(input.tools),
    toolChoice: { type: "auto" },
    generation: { maxTokens: input.maxTokens, temperature: 0.2 },
  });
}

function readAnyWorkspaceFile(
  input: Readonly<Record<string, unknown>>,
): Effect.Effect<{ readonly path: string; readonly content?: string; readonly error?: string }, never> {
  return Effect.gen(function* () {
    const path = typeof input.path === "string" ? input.path : "";
    const workspace = resolveProbeWorkspaceRoot();
    const absolutePath = resolve(workspace, path);
    const relativePath = relative(workspace, absolutePath);

    if (
      path.length === 0 ||
      path.includes("\0") ||
      relativePath.startsWith("..") ||
      relativePath === "" ||
      relativePath.split(sep).includes("..") ||
      relativePath.split(sep).includes(".git")
    ) {
      return {
        path,
        error: "path is outside the Probe workspace file scope",
      };
    }

    const content = yield* Effect.tryPromise({
      try: () => readFile(absolutePath, "utf8"),
      catch: (error) => error,
    }).pipe(
      Effect.catch((error) =>
        Effect.succeed(`failed to read ${path}: ${String(error)}`),
      ),
    );

    return {
      path,
      content: typeof content === "string" ? content.slice(0, 6000) : String(content),
    };
  });
}

function formatToolResultValue(value: { readonly type: string; readonly value: unknown }): string {
  if (value.type === "text" || value.type === "error") {
    return String(value.value);
  }

  if (isReadFileToolResult(value.value)) {
    return safeJson({
      path: value.value.path,
      contentChars: value.value.content.length,
      preview: value.value.content.slice(0, 500),
    });
  }

  return safeJson(value.value);
}

function isReadFileToolResult(value: unknown): value is { readonly path: string; readonly content: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    "path" in value &&
    typeof value.path === "string" &&
    "content" in value &&
    typeof value.content === "string"
  );
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function resolveProbeWorkspaceRoot(start = process.cwd()): string {
  let current = resolve(start);

  for (;;) {
    if (existsSync(resolve(current, "packages/runtime/src/cli.ts")) && existsSync(resolve(current, "README.md"))) {
      return current;
    }

    const parent = dirname(current);

    if (parent === current) {
      return resolve(start);
    }

    current = parent;
  }
}

if (import.meta.main) {
  const argv = Bun.argv.slice(2);

  if (argv[0] === "chat" && stringOption(parseOptions(argv.slice(1)), "prompt") === undefined) {
    process.exit(await runGeminiInteractiveChat(argv.slice(1), { env: Bun.env }));
  }

  const result = await Effect.runPromise(runProbeCli(argv, { env: Bun.env }));

  if (result.stdout.length > 0) {
    process.stdout.write(result.stdout);
  }

  if (result.stderr.length > 0) {
    process.stderr.write(result.stderr);
  }

  process.exit(result.exitCode);
}

async function runGeminiInteractiveChat(args: ReadonlyArray<string>, deps: ProbeCliDeps): Promise<number> {
  const options = parseOptions(args);
  const model = stringOption(options, "model") ?? GEMINI_DEFAULT_MODEL_ID;
  const maxTokens = numberOption(options, "max-tokens") ?? 1024;
  const tools = makeGeminiChatTools();
  const clientResult = await Effect.runPromise(
    makeGeminiClient({
      profileId: stringOption(options, "profile") ?? deps.env?.PROBE_BACKEND_PROFILE ?? GEMINI_API_PROFILE_ID,
      explicitBaseUrl: stringOption(options, "base-url"),
      env: deps.env,
      fetch: deps.fetch,
      now: deps.now,
    }).pipe(Effect.catch((error) => Effect.succeed(error))),
  );

  if (clientResult instanceof GeminiClientError || "_tag" in clientResult) {
    const message = "reason" in clientResult ? clientResult.reason : String(clientResult);
    process.stderr.write(`${message}\n`);
    return 1;
  }

  process.stdout.write([
    "Probe Gemini chat",
    `profile: ${clientResult.profile.id}`,
    `kind: ${clientResult.profile.kind}`,
    `model: ${model}`,
    `apiKeySource: ${clientResult.apiKey.source}`,
    "apiKeyRedacted: true",
    "tools: read_file,current_time",
    "Type /exit to quit.",
    "",
  ].join("\n"));

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  let messages: ReadonlyArray<ProbeLlmMessage> = [];

  try {
    for (;;) {
      const prompt = (await rl.question("probe> ")).trim();

      if (prompt.length === 0) {
        continue;
      }

      if (prompt === "/exit" || prompt === "/quit") {
        return 0;
      }

      const request = makeGeminiChatRequest({ messages, model, prompt, maxTokens, tools });
      const result = await Effect.runPromise(
        clientResult.complete({ request, tools, maxModelRoundTrips: 8 }).pipe(
          Effect.catch((error: GeminiClientError) => Effect.succeed(error)),
        ),
      );

      if (result instanceof GeminiClientError) {
        process.stdout.write(formatGeminiFailure("Probe Gemini chat", result));
        continue;
      }

      process.stdout.write(formatGeminiChatTurn({ apiKeySource: clientResult.apiKey.source, result }));
      messages = [...result.finalRequest.messages, makeProbeLlmMessage("assistant", result.text)];
    }
  } finally {
    rl.close();
  }
}
