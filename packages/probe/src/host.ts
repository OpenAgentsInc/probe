// The Node/Bun host loop: wraps the wasm Engine with stdio, a fetch-based
// transport, and tool execution. Structurally identical to probe-bin's
// native loop — commands out, events/outcomes back in — proving the
// host-boundary thesis: one core, many hosts.

import { createInterface } from "node:readline"
import { ProbeEngine, type HostCommand } from "./index.ts"
import { runOpenAiStream } from "./transport.ts"
import { defaultToolCatalog, executeTool } from "./tools.ts"

const SYSTEM_PROMPT =
  "You are Probe, the OpenAgents coding agent, working inside the session's workspace directory. " +
  "Use the available tools to inspect and change the code; keep answers concise and report what you actually did."

/** Register a secret with the scrubber applied to every outgoing line. */
class Scrubber {
  private readonly values: Array<string> = []
  register(secret: string | undefined): void {
    if (secret && secret.length >= 8 && !this.values.includes(secret)) {
      this.values.push(secret)
    }
  }
  scrub(text: string): string {
    let out = text
    for (const value of this.values) out = out.split(value).join("[redacted]")
    return out
  }
}

export async function runAcpStdio(): Promise<void> {
  const grant = process.env["PROBE_INFERENCE_GRANT"]
  const inferenceUrl = process.env["PROBE_INFERENCE_URL"]
  const transportName = process.env["PROBE_TRANSPORT"] ?? (inferenceUrl ? "openai" : "")

  const scrubber = new Scrubber()
  scrubber.register(grant)

  const catalog = defaultToolCatalog()
  const engine = new ProbeEngine({
    provider: process.env["PROBE_PROVIDER"] ?? "probe",
    model: process.env["PROBE_MODEL"] ?? "default",
    systemPrompt: process.env["PROBE_SYSTEM_PROMPT"] ?? SYSTEM_PROMPT,
    tools: catalog.definitions,
    toolKinds: catalog.kinds
  })

  const write = (line: string): void => {
    process.stdout.write(scrubber.scrub(line) + "\n")
  }

  let workspace = process.cwd()
  let cancelled = false

  const act = async (commands: ReadonlyArray<HostCommand>): Promise<void> => {
    for (const command of commands) {
      if (command.type === "write_line") {
        write(command.line)
      } else if (command.type === "start_stream") {
        cancelled = false
        void runStream(command.request)
      } else if (command.type === "cancel_stream") {
        cancelled = true
      } else if (command.type === "run_tool") {
        const result = await executeTool(command.name, command.input, workspace)
        await act(engine.onToolOutcome(command.id, scrubResult(result)))
      }
    }
  }

  const scrubResult = (result: { readonly type: string; readonly value: unknown }): unknown => {
    if (typeof result.value === "string") {
      return { type: result.type, value: scrubber.scrub(result.value) }
    }
    return result
  }

  const runStream = async (request: unknown): Promise<void> => {
    if (transportName !== "openai") {
      await act(engine.onProviderFailure(`transport ${JSON.stringify(transportName)} is not available in this host`))
      return
    }
    if (inferenceUrl && inferenceUrl.includes("openagents.com") && !grant) {
      await act(
        engine.onProviderFailure(
          "inference grant missing: PROBE_INFERENCE_URL points at a Sarah proxy but PROBE_INFERENCE_GRANT is not set"
        )
      )
      return
    }
    try {
      await runOpenAiStream({
        url: inferenceUrl ?? "",
        bearer: grant,
        request,
        isCancelled: () => cancelled,
        onEvent: (event) => act(engine.onProviderEvent(event))
      })
    } catch (error) {
      await act(engine.onProviderFailure(scrubber.scrub(String((error as Error)?.message ?? error))))
    }
  }

  const rl = createInterface({ input: process.stdin })
  for await (const line of rl) {
    if (line.trim() === "") continue
    const parsed = safeParse(line)
    if (parsed && (parsed.method === "session/new" || parsed.method === "session/load")) {
      const cwd = parsed.params?.cwd
      if (typeof cwd === "string" && cwd !== "") workspace = cwd
    }
    await act(engine.handleLine(line))
  }
}

function safeParse(line: string): { method?: string; params?: { cwd?: unknown } } | undefined {
  try {
    return JSON.parse(line)
  } catch {
    return undefined
  }
}
