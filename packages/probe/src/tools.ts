// Tool execution for the JS host. Mirrors the native probe-bin tool set,
// workspace-confined. The wasm core routes tool calls; this file runs them.
import { spawn } from "node:child_process"
import { readFile, writeFile } from "node:fs/promises"
import { isAbsolute, resolve, sep } from "node:path"

export interface ToolCatalog {
  readonly definitions: ReadonlyArray<unknown>
  readonly kinds: Readonly<Record<string, string>>
}

export function defaultToolCatalog(): ToolCatalog {
  return {
    definitions: [
      {
        name: "shell",
        description: "Run a shell command in the session workspace. Output is captured and bounded.",
        inputSchema: { type: "object", properties: { command: { type: "string" } }, required: ["command"] }
      },
      {
        name: "read_file",
        description: "Read a text file in the workspace.",
        inputSchema: { type: "object", properties: { path: { type: "string" } }, required: ["path"] }
      }
    ],
    kinds: { shell: "execute", read_file: "read" }
  }
}

const MAX_OUTPUT = 48 * 1024

function confine(root: string, raw: string): string | undefined {
  if (raw.includes("\0") || isAbsolute(raw) || raw.split(/[/\\]/).includes("..")) return undefined
  const absolute = resolve(root, raw)
  return absolute === root || absolute.startsWith(root + sep) ? absolute : undefined
}

function bound(text: string): string {
  return text.length > MAX_OUTPUT ? text.slice(0, MAX_OUTPUT) + "\n[output truncated]" : text
}

export async function executeTool(
  name: string,
  input: unknown,
  workspace: string
): Promise<{ readonly type: "text" | "error"; readonly value: string }> {
  const args = (input ?? {}) as Record<string, unknown>
  try {
    if (name === "shell") {
      const command = typeof args["command"] === "string" ? args["command"] : ""
      if (command === "") return { type: "error", value: "command is required" }
      return await runShell(command, workspace)
    }
    if (name === "read_file") {
      const path = typeof args["path"] === "string" ? confine(workspace, args["path"]) : undefined
      if (path === undefined) return { type: "error", value: "path is outside the workspace" }
      return { type: "text", value: bound(await readFile(path, "utf8")) }
    }
    return { type: "error", value: `tool not available: ${name}` }
  } catch (error) {
    return { type: "error", value: String((error as Error)?.message ?? error) }
  }
}

function runShell(command: string, workspace: string): Promise<{ type: "text" | "error"; value: string }> {
  return new Promise((resolveResult) => {
    const child = spawn("/bin/sh", ["-c", command], {
      cwd: workspace,
      env: { ...process.env, PROBE_INFERENCE_GRANT: undefined } as NodeJS.ProcessEnv,
      stdio: ["ignore", "pipe", "pipe"]
    })
    let out = ""
    let err = ""
    const timer = setTimeout(() => child.kill(), 60_000)
    child.stdout.on("data", (chunk) => (out += chunk))
    child.stderr.on("data", (chunk) => (err += chunk))
    child.on("close", (code) => {
      clearTimeout(timer)
      const combined = err ? `${out}\n[stderr]\n${err}` : out
      resolveResult(code === 0 ? { type: "text", value: bound(combined) } : { type: "error", value: bound(`exit ${code}\n${combined}`) })
    })
    child.on("error", (error) => {
      clearTimeout(timer)
      resolveResult({ type: "error", value: `failed to start shell: ${error.message}` })
    })
  })
}
