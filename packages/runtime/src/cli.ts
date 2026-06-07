#!/usr/bin/env bun
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { Effect, Schema as S } from "effect";
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

    return {
      exitCode: 1,
      stdout: usage(),
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
  ].join("\n") + "\n";
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
