#!/usr/bin/env bun
import { existsSync } from "node:fs";
import { mkdir, readFile, readdir, stat, writeFile } from "node:fs/promises";
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
  type ProbeLlmEvent,
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
  readonly colors?: boolean;
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
    const tools = makeGeminiChatTools(deps.env);
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
        stdout: formatGeminiFailure("Probe Gemini chat", result, makeCliColors(options, deps)),
        stderr: "",
      };
    }

    return {
      exitCode: 0,
      stdout: formatGeminiChatTurn({
        apiKeySource: client.apiKey.source,
        colors: makeCliColors(options, deps),
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
        stdout: formatGeminiFailure(input.title, result, makeCliColors(input.options, input.deps)),
        stderr: "",
      };
    }

    return {
      exitCode: 0,
      stdout: formatGeminiCompletion(input.title, client.apiKey.source, result, makeCliColors(input.options, input.deps)),
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
    "  probe chat [--profile gemini-api] [--model gemini-2.5-flash] [--prompt TEXT] [--color always|never] [--no-color]",
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

function formatGeminiCompletion(
  title: string,
  apiKeySource: string,
  completion: GeminiCompleteResult,
  colors: ProbeCliColors = noCliColors,
): string {
  return [
    cliHeader(colors, title),
    cliField(colors, "profile", completion.profile.id),
    cliField(colors, "kind", completion.profile.kind),
    cliField(colors, "model", completion.finalRequest.model.model),
    cliField(colors, "apiKeySource", apiKeySource),
    cliField(colors, "apiKeyRedacted", "true"),
    cliLine(colors, "assistant", completion.text, "assistant"),
    cliField(colors, "roundTrips", String(completion.roundTrips), "muted"),
    cliLine(colors, "usage", formatGeminiUsage(completion.receipt.usage), "usage"),
    cliField(colors, "receipt", JSON.stringify(completion.receipt), "muted"),
  ].join("\n") + "\n";
}

function formatGeminiChatTurn(input: {
  readonly apiKeySource: string;
  readonly colors?: ProbeCliColors;
  readonly includeHeader?: boolean;
  readonly result: GeminiCompleteResult;
}): string {
  const lines: string[] = [];
  const colors = input.colors ?? noCliColors;

  if (input.includeHeader === true) {
    lines.push(cliHeader(colors, "Probe Gemini chat"));
    lines.push(cliField(colors, "profile", input.result.profile.id));
    lines.push(cliField(colors, "kind", input.result.profile.kind));
    lines.push(cliField(colors, "model", input.result.finalRequest.model.model));
    lines.push(cliField(colors, "apiKeySource", input.apiKeySource));
    lines.push(cliField(colors, "apiKeyRedacted", "true"));
  }

  for (const event of input.result.events) {
    if (event.type === "tool-call") {
      lines.push(cliToolLine(colors, "tool_call", event.name, safeJson(event.input), "call"));
    }

    if (event.type === "tool-result") {
      lines.push(cliToolLine(colors, "tool_result", event.name, formatToolResultValue(event.result), "result"));
    }

    if (event.type === "tool-error") {
      lines.push(cliToolLine(colors, "tool_error", event.name, event.message, "error"));
    }
  }

  lines.push(cliLine(colors, "assistant", input.result.text, "assistant"));
  lines.push(cliField(colors, "roundTrips", String(input.result.roundTrips), "muted"));
  lines.push(cliLine(colors, "usage", formatGeminiUsage(input.result.receipt.usage), "usage"));

  return `${lines.join("\n")}\n`;
}

function formatGeminiFailure(title: string, error: GeminiClientError, colors: ProbeCliColors = noCliColors): string {
  const lines = [
    cliHeader(colors, `${title} failed`, "error"),
    cliField(colors, "failureClass", error.failureClass, "error"),
    cliField(colors, "message", error.reason, "error"),
  ];

  if (error.receipt !== undefined) {
    lines.push(cliField(colors, "receipt", JSON.stringify(error.receipt), "muted"));
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

type ProbeCliColorRole = "assistant" | "default" | "error" | "header" | "muted" | "prompt" | "tool" | "usage";

interface ProbeCliColors {
  readonly enabled: boolean;
}

const noCliColors: ProbeCliColors = { enabled: false };

const ansi = {
  reset: "\x1b[0m",
  bold: "\x1b[1m",
  dim: "\x1b[2m",
  cyan: "\x1b[36m",
  green: "\x1b[32m",
  yellow: "\x1b[33m",
  magenta: "\x1b[35m",
  blue: "\x1b[34m",
  red: "\x1b[31m",
  gray: "\x1b[90m",
} as const;

function makeCliColors(
  options: Record<string, string | true>,
  deps: ProbeCliDeps,
  defaultEnabled = false,
): ProbeCliColors {
  return {
    enabled: shouldUseCliColors(options, deps.env, deps.colors ?? defaultEnabled),
  };
}

function shouldUseCliColors(
  options: Record<string, string | true>,
  env: Readonly<Record<string, string | undefined>> = {},
  defaultEnabled = false,
): boolean {
  const option = stringOption(options, "color");

  if (option === "always") {
    return true;
  }

  if (option === "never" || options["no-color"] === true || env.PROBE_NO_COLOR !== undefined || env.NO_COLOR !== undefined) {
    return false;
  }

  if (env.PROBE_COLOR === "always" || (env.FORCE_COLOR !== undefined && env.FORCE_COLOR !== "0")) {
    return true;
  }

  if (env.PROBE_COLOR === "never" || env.TERM === "dumb") {
    return false;
  }

  return defaultEnabled;
}

function cliHeader(colors: ProbeCliColors, value: string, role: ProbeCliColorRole = "header"): string {
  return cliColor(colors, role, value);
}

function cliField(
  colors: ProbeCliColors,
  label: string,
  value: string,
  role: ProbeCliColorRole = "default",
): string {
  return `${cliLabel(colors, label, role)} ${cliColor(colors, role === "error" ? "error" : "muted", value)}`;
}

function cliLine(
  colors: ProbeCliColors,
  label: string,
  value: string,
  role: ProbeCliColorRole = "default",
): string {
  return `${cliLabel(colors, label, role)} ${value}`;
}

function cliToolLine(
  colors: ProbeCliColors,
  label: string,
  name: string,
  value: string,
  kind: "call" | "error" | "result",
): string {
  const role = kind === "error" ? "error" : "tool";

  return `${cliLabel(colors, label, role)} ${cliColor(colors, "tool", name)} ${cliColor(colors, kind === "call" ? "muted" : role, value)}`;
}

function cliLabel(colors: ProbeCliColors, label: string, role: ProbeCliColorRole): string {
  return cliColor(colors, role, `${label}:`);
}

function cliColor(colors: ProbeCliColors, role: ProbeCliColorRole, value: string): string {
  if (!colors.enabled) {
    return value;
  }

  const code = role === "assistant"
    ? `${ansi.bold}${ansi.green}`
    : role === "error"
      ? `${ansi.bold}${ansi.red}`
      : role === "header"
        ? `${ansi.bold}${ansi.cyan}`
        : role === "muted"
          ? ansi.gray
          : role === "prompt"
            ? `${ansi.bold}${ansi.cyan}`
            : role === "tool"
              ? ansi.magenta
              : role === "usage"
                ? ansi.yellow
                : ansi.cyan;

  return `${code}${value}${ansi.reset}`;
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

function makeGeminiChatTools(env: Readonly<Record<string, string | undefined>> = {}): ProbeLlmTools {
  return {
    read_file: defineProbeLlmTool({
      name: "read_file",
      description: "Read a UTF-8 text file under the OpenAgents workspace. Use this when the user asks about local code or reference repos.",
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
      execute: (input) => readAnyWorkspaceFile(input, env),
    }),
    write_file: defineProbeLlmTool({
      name: "write_file",
      description: "Write a UTF-8 text file under the OpenAgents workspace. Creates parent directories if needed. Use this to create new files or overwrite existing ones.",
      inputSchema: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "A relative file path under the workspace.",
          },
          content: {
            type: "string",
            description: "The full file content to write.",
          },
        },
        required: ["path", "content"],
      },
      execute: (input) => writeAnyWorkspaceFile(input, env),
    }),
    list_files: defineProbeLlmTool({
      name: "list_files",
      description: "List files under a directory in the OpenAgents workspace.",
      inputSchema: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "A relative directory path under the workspace.",
          },
          limit: {
            type: "number",
            description: "Maximum files to return.",
          },
        },
      },
      execute: (input) => listWorkspaceFiles(input, env),
    }),
    search_code: defineProbeLlmTool({
      name: "search_code",
      description: "Search text in files under the OpenAgents workspace using ripgrep.",
      inputSchema: {
        type: "object",
        properties: {
          query: {
            type: "string",
            description: "The literal text or regex pattern to search for.",
          },
          path: {
            type: "string",
            description: "Optional relative directory or file path to search under.",
          },
          limit: {
            type: "number",
            description: "Maximum matching lines to return.",
          },
        },
        required: ["query"],
      },
      execute: (input) => searchWorkspaceCode(input, env),
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
      "You are Probe, a concise coding agent. You can inspect the local OpenAgents workspace, including sibling repos and reference repos such as projects/repos/opencode, through tools. " +
      "Use list_files, search_code, and read_file when the user asks about local code. Do not refuse local workspace inspection just because the path is outside the Probe package. " +
      "When you use a tool, continue to a direct final answer after the tool result.",
    messages: [...input.messages, makeProbeLlmMessage("user", input.prompt)],
    tools: probeLlmToolDefinitions(input.tools),
    toolChoice: { type: "auto" },
    generation: { maxTokens: input.maxTokens, temperature: 0.2 },
  });
}

function readAnyWorkspaceFile(
  input: Readonly<Record<string, unknown>>,
  env: Readonly<Record<string, string | undefined>> = {},
): Effect.Effect<{ readonly path: string; readonly content?: string; readonly error?: string }, never> {
  return Effect.gen(function* () {
    const path = typeof input.path === "string" ? input.path : "";
    const workspace = resolveProbeChatWorkspaceRoot(env);
    const resolved = resolveWorkspacePath(workspace, path);

    if (resolved === undefined) {
      return {
        path,
        error: "path is outside the OpenAgents workspace file scope",
      };
    }

    const content = yield* Effect.tryPromise({
      try: () => readFile(resolved.absolutePath, "utf8"),
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

function writeAnyWorkspaceFile(
  input: Readonly<Record<string, unknown>>,
  env: Readonly<Record<string, string | undefined>> = {},
): Effect.Effect<{ readonly path: string; readonly content?: string; readonly error?: string }, never> {
  return Effect.gen(function* () {
    const path = typeof input.path === "string" ? input.path : "";
    const content = typeof input.content === "string" ? input.content : "";
    const workspace = resolveProbeChatWorkspaceRoot(env);
    const resolved = resolveWorkspacePath(workspace, path);

    if (resolved === undefined) {
      return { path, error: "path is outside the workspace file scope" };
    }

    if (!content) {
      return { path, error: "content is required" };
    }

    yield* Effect.tryPromise({
      try: () => mkdir(dirname(resolved.absolutePath), { recursive: true }),
      catch: (error) => error,
    }).pipe(
      Effect.catch((error) =>
        Effect.succeed(void 0),
      ),
    );

    const written = yield* Effect.tryPromise({
      try: () => writeFile(resolved.absolutePath, content, "utf8").then(() => true),
      catch: (error) => error,
    }).pipe(
      Effect.catch((error) =>
        Effect.succeed(`failed to write ${path}: ${String(error)}`),
      ),
    );

    if (written === true) {
      return { path, content: `written to ${resolved.relativePath}` };
    }

    return { path, error: written };
  });
}

function listWorkspaceFiles(
  input: Readonly<Record<string, unknown>>,
  env: Readonly<Record<string, string | undefined>> = {},
): Effect.Effect<{
  readonly path: string;
  readonly directories?: ReadonlyArray<string>;
  readonly files?: ReadonlyArray<string>;
  readonly truncated?: boolean;
  readonly error?: string;
}, never> {
  return Effect.gen(function* () {
    const path = typeof input.path === "string" ? input.path : ".";
    const limit = boundedToolLimit(input.limit, 80);
    const workspace = resolveProbeChatWorkspaceRoot(env);
    const resolved = resolveWorkspacePath(workspace, path);

    if (resolved === undefined) {
      return { path, error: "path is outside the OpenAgents workspace file scope" };
    }

    const listing = yield* collectWorkspaceEntries(resolved.absolutePath, resolved.relativePath, limit).pipe(
      Effect.catch((error) => Effect.succeed({ directories: [], files: [`failed to list ${path}: ${String(error)}`], truncated: false })),
    );

    return {
      path,
      ...listing,
    };
  });
}

function searchWorkspaceCode(
  input: Readonly<Record<string, unknown>>,
  env: Readonly<Record<string, string | undefined>> = {},
): Effect.Effect<{ readonly query: string; readonly path: string; readonly matches?: ReadonlyArray<string>; readonly truncated?: boolean; readonly error?: string }, never> {
  return Effect.gen(function* () {
    const query = typeof input.query === "string" ? input.query : "";
    const path = typeof input.path === "string" ? input.path : ".";
    const limit = boundedToolLimit(input.limit, 80);
    const workspace = resolveProbeChatWorkspaceRoot(env);
    const resolved = resolveWorkspacePath(workspace, path);

    if (query.length === 0) {
      return { query, path, error: "query is required" };
    }

    if (resolved === undefined) {
      return { query, path, error: "path is outside the OpenAgents workspace file scope" };
    }

    const output = yield* Effect.tryPromise({
      try: async () => {
        const child = Bun.spawn(["rg", "--line-number", "--no-heading", "--color", "never", query, resolved.relativePath], {
          cwd: workspace,
          stderr: "pipe",
          stdout: "pipe",
        });
        const text = await new Response(child.stdout).text();
        const errorText = await new Response(child.stderr).text();
        const exitCode = await child.exited;

        if (exitCode > 1) {
          return `ripgrep failed: ${errorText.trim()}`;
        }

        return text;
      },
      catch: (error) => `failed to search ${path}: ${String(error)}`,
    });
    const allMatches = output.split("\n").filter((line) => line.length > 0);
    const matches = allMatches.slice(0, limit);

    return {
      query,
      path,
      matches,
      truncated: allMatches.length > matches.length,
    };
  });
}

function collectWorkspaceEntries(
  absolutePath: string,
  relativePath: string,
  limit: number,
): Effect.Effect<{
  readonly directories: ReadonlyArray<string>;
  readonly files: ReadonlyArray<string>;
  readonly truncated: boolean;
}, unknown> {
  return Effect.tryPromise(async () => {
    const rootStat = await stat(absolutePath);

    if (rootStat.isFile()) {
      return { directories: [], files: [relativePath], truncated: false };
    }

    const files: string[] = [];
    const directories: string[] = [];
    const entries = await readdir(absolutePath, { withFileTypes: true });
    const visibleEntries = entries.filter((entry) => !shouldSkipWorkspaceEntry(entry.name));

    for (const entry of visibleEntries) {
      if (directories.length + files.length >= limit) {
        break;
      }

      const entryRelativePath = relativePath === "." ? entry.name : `${relativePath}/${entry.name}`;

      if (entry.isDirectory()) {
        directories.push(entryRelativePath);
        continue;
      }

      if (entry.isFile()) {
        files.push(entryRelativePath);
      }
    }

    return {
      directories,
      files,
      truncated: visibleEntries.length > directories.length + files.length,
    };
  });
}

function resolveProbeChatWorkspaceRoot(env: Readonly<Record<string, string | undefined>> = {}): string {
  return resolve(env.PROBE_WORKSPACE_ROOT ?? env.OPENAGENTS_WORKSPACE_ROOT ?? dirname(resolveProbeWorkspaceRoot()));
}

function resolveWorkspacePath(
  workspace: string,
  path: string,
): { readonly absolutePath: string; readonly relativePath: string } | undefined {
  const absolutePath = resolve(workspace, path);
  const relativePath = relative(workspace, absolutePath) || ".";

  if (
    path.length === 0 ||
    path.includes("\0") ||
    relativePath.startsWith("..") ||
    relativePath.split(sep).includes("..") ||
    relativePath.split(sep).includes(".git")
  ) {
    return undefined;
  }

  return { absolutePath, relativePath };
}

function shouldSkipWorkspaceEntry(name: string): boolean {
  return name === ".git" || name === "node_modules" || name === ".next" || name === "dist" || name === "build";
}

function boundedToolLimit(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.min(Math.floor(value), 500) : fallback;
}

function makeGeminiInteractiveTurnStream(colors: ProbeCliColors): {
  readonly onEvent: (event: ProbeLlmEvent) => void;
  readonly finish: (result?: GeminiCompleteResult) => void;
} {
  let textOpen = false;
  let sawText = false;

  const closeText = () => {
    if (textOpen) {
      process.stdout.write("\n");
      textOpen = false;
    }
  };

  return {
    onEvent: (event) => {
      if (event.type === "text-delta") {
        if (!textOpen) {
          process.stdout.write(`${cliLabel(colors, "assistant", "assistant")} `);
          textOpen = true;
        }

        sawText = true;
        process.stdout.write(event.text);
        return;
      }

      if (event.type === "tool-call") {
        closeText();
        process.stdout.write(`${cliToolLine(colors, "tool_call", event.name, safeJson(event.input), "call")}\n`);
        return;
      }

      if (event.type === "tool-result") {
        closeText();
        process.stdout.write(`${cliToolLine(colors, "tool_result", event.name, formatToolResultValue(event.result), "result")}\n`);
        return;
      }

      if (event.type === "tool-error") {
        closeText();
        process.stdout.write(`${cliToolLine(colors, "tool_error", event.name, event.message, "error")}\n`);
      }
    },
    finish: (result) => {
      closeText();

      if (result === undefined) {
        return;
      }

      if (!sawText && result.text.length > 0) {
        process.stdout.write(`${cliLine(colors, "assistant", result.text, "assistant")}\n`);
      }

      process.stdout.write(`${cliField(colors, "roundTrips", String(result.roundTrips), "muted")}  ${cliLine(colors, "usage", formatGeminiUsage(result.receipt.usage), "usage")}\n`);
    },
  };
}

function formatToolResultValue(value: { readonly type: string; readonly value: unknown }): string {
  if (value.type === "error") {
    return String(value.value);
  }

  if (isReadFileToolResult(value.value)) {
    const r = value.value;
    return `${r.path}  (${r.content.length} chars)`;
  }

  if (isListFilesToolResult(value.value)) {
    const r = value.value;
    const parts: Array<string> = [];
    if (r.directories.length > 0) parts.push(`${r.directories.length} dirs`);
    if (r.files.length > 0) parts.push(`${r.files.length} files`);
    if (r.truncated) parts.push("truncated");
    return `${r.path}  ${parts.length > 0 ? parts.join(", ") : "empty"}`;
  }

  if (isSearchCodeToolResult(value.value)) {
    const r = value.value;
    const label = `${r.matches.length} match${r.matches.length === 1 ? "" : "es"}`;
    return `${r.query}  in  ${r.path}  (${label}${r.truncated ? ", truncated" : ""})`;
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

function isListFilesToolResult(value: unknown): value is {
  readonly path: string;
  readonly directories: ReadonlyArray<string>;
  readonly files: ReadonlyArray<string>;
  readonly truncated?: boolean;
} {
  return (
    typeof value === "object" &&
    value !== null &&
    "path" in value &&
    typeof value.path === "string" &&
    "directories" in value &&
    Array.isArray(value.directories) &&
    "files" in value &&
    Array.isArray(value.files)
  );
}

function isSearchCodeToolResult(value: unknown): value is {
  readonly query: string;
  readonly path: string;
  readonly matches: ReadonlyArray<string>;
  readonly truncated?: boolean;
} {
  return (
    typeof value === "object" &&
    value !== null &&
    "query" in value &&
    typeof value.query === "string" &&
    "path" in value &&
    typeof value.path === "string" &&
    "matches" in value &&
    Array.isArray(value.matches)
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
    process.exit(await runGeminiInteractiveChat(argv.slice(1), { colors: process.stdout.isTTY, env: Bun.env }));
  }

  const result = await Effect.runPromise(runProbeCli(argv, { colors: process.stdout.isTTY, env: Bun.env }));

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
  const tools = makeGeminiChatTools(deps.env);
  const colors = makeCliColors(options, deps);
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
    process.stderr.write(`${cliColor(colors, "error", message)}\n`);
    return 1;
  }

  process.stdout.write(
    `${cliField(colors, "profile", clientResult.profile.id)}  ${cliField(colors, "kind", clientResult.profile.kind)}  ${cliField(colors, "model", model)}  ${cliField(colors, "tools", "read_file,write_file,list_files,search_code,current_time", "tool")}\n`,
  );

  const rl = createInterface({ input: process.stdin, output: process.stdout });
  let messages: ReadonlyArray<ProbeLlmMessage> = [];

  try {
    for (;;) {
      const prompt = (await rl.question(cliColor(colors, "prompt", "probe> "))).trim();

      if (prompt.length === 0) {
        continue;
      }

      if (prompt === "/exit" || prompt === "/quit") {
        return 0;
      }

      const request = makeGeminiChatRequest({ messages, model, prompt, maxTokens, tools });
      const stream = makeGeminiInteractiveTurnStream(colors);
      const result = await Effect.runPromise(
        clientResult.complete({ request, tools, maxModelRoundTrips: 8, onEvent: stream.onEvent }).pipe(
          Effect.catch((error: GeminiClientError) => Effect.succeed(error)),
        ),
      );

      if (result instanceof GeminiClientError) {
        stream.finish();
        process.stdout.write(formatGeminiFailure("Probe Gemini chat", result, colors));
        continue;
      }

      stream.finish(result);
      messages = [...result.finalRequest.messages, makeProbeLlmMessage("assistant", result.text)];
    }
  } finally {
    rl.close();
  }
}
