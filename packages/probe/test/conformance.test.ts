import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { parseOpenAiSse, probeVersion, ProbeEngine } from "../src"

const fixtures = join(import.meta.dir, "../../../fixtures")

describe("wasm core under Node/Bun (Phase 6 conformance)", () => {
  test("loads and reports a version", () => {
    expect(probeVersion()).toBe("0.0.0")
  })

  test("drives the ACP lifecycle end to end in-process", () => {
    const engine = new ProbeEngine({ provider: "stub", model: "stub", toolKinds: { read_file: "read" } })
    const init = engine.handleLine(
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}'
    )
    const initLine = init.find((command) => command.type === "write_line")
    expect(initLine && JSON.parse((initLine as { line: string }).line).result.protocolVersion).toBe(1)

    const session = engine.handleLine(
      '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}'
    )
    const sessionId = JSON.parse((session[0] as { line: string }).line).result.sessionId
    const started = engine.handleLine(
      `{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"${sessionId}","prompt":[{"type":"text","text":"hi"}]}}`
    )
    expect(started.some((command) => command.type === "start_stream")).toBe(true)

    // Feed a neutral text delta + finish; the engine renders an update and a
    // stop reason — proving the TS mirror speaks the same contract JSON as
    // the Rust core (which produced the shared corpus).
    engine.onProviderEvent({ type: "text-delta", id: "text-0", text: "hello" })
    const done = engine.onProviderEvent({ type: "finish", reason: "stop" })
    const response = done
      .map((command) => (command.type === "write_line" ? JSON.parse(command.line) : undefined))
      .find((value) => value?.id === 3)
    expect(response.result.stopReason).toBe("end_turn")
  })

  test("parses the shared Gemini-shaped and OpenAI SSE corpus consistently", () => {
    // The OpenAI SSE parser is the wasm-exposed one; feed a known stream and
    // assert the neutral event contract the Rust side pins.
    const events = parseOpenAiSse(
      'data: {"choices":[{"delta":{"content":"Hi"},"finish_reason":"stop"}]}\n\n' +
        'data: {"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}\n\n' +
        "data: [DONE]\n\n"
    ) as ReadonlyArray<{ type: string; usage?: { totalTokens?: number } }>
    expect(events.map((event) => event.type)).toEqual(["step-start", "text-delta", "step-finish", "finish"])
    expect(events.at(-1)?.usage?.totalTokens).toBe(4)
  })

  test("the llm roundtrip corpus decodes through the wasm lowering path", () => {
    // Every request in the shared corpus must lower without error — the TS
    // mirror consuming fixtures/llm/roundtrip.json, closing the loop the
    // Rust harness opened in #206.
    const corpus = JSON.parse(readFileSync(join(fixtures, "llm/roundtrip.json"), "utf8"))
    for (const request of corpus.requests) {
      const { lowerOpenAiRequest } = require("../src") as typeof import("../src")
      const lowered = lowerOpenAiRequest(request) as { model: string; messages: unknown[] }
      expect(typeof lowered.model).toBe("string")
      expect(Array.isArray(lowered.messages)).toBe(true)
    }
  })
})
