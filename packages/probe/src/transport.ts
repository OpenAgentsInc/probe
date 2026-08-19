// The fetch-based OpenAI-compatible transport. The wasm core does the SSE
// parsing (parseOpenAiSse); this host owns only the streaming HTTP request
// and incremental line delivery with cancellation between chunks.
import { loadProbeWasm } from "./index.ts"

export interface OpenAiStreamOptions {
  readonly url: string
  readonly bearer?: string | undefined
  readonly request: unknown
  readonly isCancelled: () => boolean
  readonly onEvent: (event: unknown) => Promise<void>
}

function chatUrl(base: string): string {
  return base.includes("/chat/completions") ? base : `${base.replace(/\/+$/, "")}/chat/completions`
}

export async function runOpenAiStream(options: OpenAiStreamOptions): Promise<void> {
  const wasm = loadProbeWasm()
  const body = wasm.lowerOpenAiRequest(JSON.stringify(options.request))
  const headers: Record<string, string> = { "content-type": "application/json" }
  if (options.bearer) headers["authorization"] = `Bearer ${options.bearer}`

  const response = await fetch(chatUrl(options.url), { method: "POST", headers, body })
  if (!response.ok || response.body === null) {
    throw new Error(`provider returned HTTP ${response.status}`)
  }

  // Feed complete SSE "data:" lines through the wasm parser incrementally,
  // observing cancellation between reads. The parser holds its own state
  // across calls via a fresh instance per stream; here we accumulate and
  // parse the whole body once the stream ends or is cancelled, matching the
  // native transport's line loop closely enough for the neutral events.
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  for (;;) {
    if (options.isCancelled()) return
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
  }
  const events = JSON.parse(wasm.parseOpenAiSse(buffer)) as ReadonlyArray<unknown>
  for (const event of events) {
    if (options.isCancelled()) return
    await options.onEvent(event)
  }
}
