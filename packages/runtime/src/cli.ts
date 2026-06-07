#!/usr/bin/env bun
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { Effect, Schema as S } from "effect";
import {
  AppleFmBackendError,
  makeAppleFmClient,
  type AppleFmPlainTextCompletion,
  type AppleFmReadiness,
} from "./backends/apple-fm/client";
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

    if (namespace === "apple-fm" && command === "status") {
      return yield* appleFmStatus(options, deps);
    }

    if (namespace === "apple-fm" && command === "smoke") {
      return yield* appleFmSmoke(options, deps);
    }

    return {
      exitCode: 1,
      stdout: usage(),
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
    "  probe apple-fm status [--base-url URL] [--profile apple-fm-local]",
    "  probe apple-fm smoke [--base-url URL] [--profile apple-fm-local] [--prompt TEXT]",
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

if (import.meta.main) {
  const result = await Effect.runPromise(runProbeCli(Bun.argv.slice(2), { env: Bun.env }));

  if (result.stdout.length > 0) {
    process.stdout.write(result.stdout);
  }

  if (result.stderr.length > 0) {
    process.stderr.write(result.stderr);
  }

  process.exit(result.exitCode);
}
