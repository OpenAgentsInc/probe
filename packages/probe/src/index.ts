// @openagentsinc/probe: the npm package. A platform-neutral wasm core (async
// ABI, no JSPI) wrapped by a thin JS host that owns fetch, tool execution,
// and stdio — the same Engine that probe-bin drives natively. Runs under
// Node LTS and Bun with no flags; the controller pins it as a default
// catalog agent spawned via [process.execPath, binPath] (spec Addendum A3).

// The wasm-bindgen nodejs target is CommonJS; import it through createRequire
// so this ESM entry stays dependency-light.
import { createRequire } from "node:module"

const require_ = createRequire(import.meta.url)

export type HostCommand =
  | { readonly type: "write_line"; readonly line: string }
  | { readonly type: "start_stream"; readonly request: unknown }
  | { readonly type: "cancel_stream" }
  | { readonly type: "run_tool"; readonly id: string; readonly name: string; readonly input: unknown }

export interface ProbeEngineConfig {
  readonly provider?: string
  readonly model?: string
  readonly systemPrompt?: string
  readonly tools?: ReadonlyArray<unknown>
  readonly toolKinds?: Readonly<Record<string, string>>
}

interface WasmEngine {
  handleLine(line: string): string
  onProviderEvent(eventJson: string): string
  onProviderFailure(message: string): string
  onToolOutcome(toolCallId: string, resultJson: string): string
}

interface WasmModule {
  ProbeEngine: new (configJson: string) => WasmEngine
  probeVersion(): string
  lowerOpenAiRequest(requestJson: string): string
  parseOpenAiSse(body: string): string
}

let cached: WasmModule | undefined

/** Load the wasm core (cached). The nodejs bindings instantiate eagerly. */
export function loadProbeWasm(): WasmModule {
  if (cached === undefined) {
    cached = require_("../wasm/probe_wasm.cjs") as WasmModule
  }
  return cached
}

export function probeVersion(): string {
  return loadProbeWasm().probeVersion()
}

/**
 * A thin, typed wrapper over the wasm engine. Every method returns the
 * decoded host-command array the caller must act on — write the lines to the
 * client, run the model transport, execute tools — exactly as probe-bin's
 * loop does natively.
 */
export class ProbeEngine {
  private readonly inner: WasmEngine

  constructor(config: ProbeEngineConfig = {}) {
    const wasm = loadProbeWasm()
    this.inner = new wasm.ProbeEngine(
      JSON.stringify({
        provider: config.provider ?? "probe",
        model: config.model ?? "default",
        systemPrompt: config.systemPrompt ?? "",
        tools: config.tools ?? [],
        toolKinds: config.toolKinds ?? {}
      })
    )
  }

  handleLine(line: string): ReadonlyArray<HostCommand> {
    return JSON.parse(this.inner.handleLine(line)) as ReadonlyArray<HostCommand>
  }

  onProviderEvent(event: unknown): ReadonlyArray<HostCommand> {
    return JSON.parse(this.inner.onProviderEvent(JSON.stringify(event))) as ReadonlyArray<HostCommand>
  }

  onProviderFailure(message: string): ReadonlyArray<HostCommand> {
    return JSON.parse(this.inner.onProviderFailure(message)) as ReadonlyArray<HostCommand>
  }

  onToolOutcome(toolCallId: string, result: unknown): ReadonlyArray<HostCommand> {
    return JSON.parse(this.inner.onToolOutcome(toolCallId, JSON.stringify(result))) as ReadonlyArray<HostCommand>
  }
}

/** Pure lowerings/parsers exposed for hosts that run their own fetch. */
export function lowerOpenAiRequest(request: unknown): unknown {
  return JSON.parse(loadProbeWasm().lowerOpenAiRequest(JSON.stringify(request)))
}

export function parseOpenAiSse(body: string): ReadonlyArray<unknown> {
  return JSON.parse(loadProbeWasm().parseOpenAiSse(body)) as ReadonlyArray<unknown>
}
