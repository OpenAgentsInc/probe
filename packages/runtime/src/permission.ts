import { Effect } from "effect";
import { createDiffPreview } from "./file-mutation";

// ── Types ─────────────────────────────────────────────────────────────────

export interface PermissionRequest {
  readonly action: "edit" | "write" | "delete";
  readonly filePath: string;
  readonly diff: string;
}

export type PermissionDecision = "allow" | "deny" | "always";

export interface PermissionHandler {
  ask(request: PermissionRequest): Effect.Effect<PermissionDecision, never>;
}

// ── Module-level permission handler ───────────────────────────────────────

let currentHandler: PermissionHandler = {
  ask: () => Effect.succeed("allow"),
};

export function setPermissionHandler(handler: PermissionHandler): void {
  currentHandler = handler;
}

export function getPermissionHandler(): PermissionHandler {
  return currentHandler;
}

export function resetPermissionHandler(): void {
  currentHandler = { ask: () => Effect.succeed("allow") };
}

// ── Pre-built handlers ────────────────────────────────────────────────────

export function makeCliPermissionHandler(askUser: (prompt: string) => Promise<boolean>): PermissionHandler {
  return {
    ask: (request: PermissionRequest) =>
      Effect.gen(function* () {
        const header = `\n[Permission] ${request.action} ${request.filePath}`;
        const sep = "-".repeat(Math.min(header.length, 60));
        const preview = createDiffPreview(
          request.diff,
          "",
        );
        const diffLines = preview.split("\n").filter((l) => l.startsWith("-") || l.startsWith("+") || l.startsWith("---") || l.startsWith("+++"));
        const allowed = yield* Effect.tryPromise({
          try: () =>
            askUser(
              `${header}\n${sep}\nChanges:\n${diffLines.slice(0, 20).join("\n")}\n${sep}\nAllow? (y=yes, n=no, a=always): `,
            ),
          catch: () => false,
        });
        if (allowed === true) return "allow" as const;
        return "deny" as const;
      }),
  };
}

export function makeInteractivePermissionHandler(): PermissionHandler {
  const rl = { value: null as import("node:readline/promises").Interface | null };
  return {
    ask: (request: PermissionRequest) =>
      Effect.gen(function* () {
        const { createInterface } = yield* Effect.sync(() => import("node:readline/promises"));
        if (rl.value === null) {
          rl.value = createInterface({ input: process.stdin, output: process.stdout });
        }
        const header = `\n[Permission] ${request.action} ${request.filePath}`;
        const sep = "-".repeat(Math.min(header.length, 60));
        const diffLines = request.diff.split("\n").slice(0, 20);
        const answer = yield* Effect.tryPromise({
          try: () => rl.value!.question(`${header}\n${sep}\n${diffLines.join("\n")}\n${sep}\nAllow? (y=yes, n=no, a=always): `),
          catch: () => "n",
        });
        const trimmed = answer.trim().toLowerCase();
        if (trimmed === "a") return "always" as const;
        if (trimmed === "y" || trimmed === "yes") return "allow" as const;
        return "deny" as const;
      }),
  };
}
