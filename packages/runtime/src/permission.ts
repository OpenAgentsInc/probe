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
  return {
    ask: (request: PermissionRequest) =>
      Effect.gen(function* () {
        const header = `\n[Permission] ${request.action} ${request.filePath}`;
        const sep = "-".repeat(Math.min(header.length, 60));
        const diffLines = request.diff.split("\n").slice(0, 20);
        const prompt = `${header}\n${sep}\n${diffLines.join("\n")}\n${sep}\nAllow? (y=yes, n=no, a=always): `;

        const answer = yield* Effect.tryPromise({
          try: () => readLineFromStdin(prompt),
          catch: () => "n" as const,
        });

        const trimmed = answer.trim().toLowerCase();
        if (trimmed === "a") return "always" as const;
        if (trimmed === "y" || trimmed === "yes") return "allow" as const;
        return "deny" as const;
      }),
  };
}

function readLineFromStdin(prompt: string): Promise<string> {
  return new Promise((resolve) => {
    const wasRaw = process.stdin.isRaw;
    if (wasRaw && typeof process.stdin.setRawMode === "function") {
      process.stdin.setRawMode(false);
    }

    process.stderr.write(prompt);

    let buffer = "";
    const onData = (chunk: Buffer) => {
      for (const byte of chunk) {
        if (byte === 0x0a) {
          cleanup();
          resolve(buffer);
          return;
        }
        if (byte === 0x0d) {
          cleanup();
          resolve(buffer);
          return;
        }
        if (byte >= 0x20) {
          buffer += String.fromCodePoint(byte);
        }
      }
    };

    const cleanup = () => {
      process.stdin.removeListener("data", onData);
      if (wasRaw && typeof process.stdin.setRawMode === "function" && !process.stdin.isRaw) {
        process.stdin.setRawMode(true);
      }
    };

    process.stdin.on("data", onData);
  });
}
