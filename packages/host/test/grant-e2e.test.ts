import { describe, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { createServer } from "node:http"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Effect } from "effect"
import { InferenceGrantRef, literalBroker, ProviderAccountRef, ProviderSecretRef, ResolvedInferenceGrant, withMaterializedGrant } from "../src"

const PROBE_BIN = process.env["PROBE_BIN"] ?? join(import.meta.dir, "../../../target/debug/probe-bin")

/** A fake Sarah proxy that records the bearer it saw and streams one reply. */
function fakeProxy(): Promise<{ url: string; bearer: () => string | undefined; close: () => void }> {
  return new Promise((resolveServer) => {
    let seenBearer: string | undefined
    const server = createServer((request, response) => {
      seenBearer = request.headers["authorization"]
      let body = ""
      request.on("data", (chunk) => (body += chunk))
      request.on("end", () => {
        response.writeHead(200, { "content-type": "text/event-stream" })
        response.write(`data: {"choices":[{"delta":{"content":"grant accepted"},"finish_reason":"stop"}]}\n\n`)
        response.write(`data: {"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}\n\n`)
        response.write("data: [DONE]\n\n")
        response.end()
      })
    })
    server.listen(0, "127.0.0.1", () => {
      const address = server.address()
      const port = typeof address === "object" && address ? address.port : 0
      resolveServer({
        url: `http://127.0.0.1:${port}/chat/completions`,
        bearer: () => seenBearer,
        close: () => server.close()
      })
    })
  })
}

describe("controller-shaped grant -> probe delegation", () => {
  test("a materialized grant launches probe against the proxy as bearer", async () => {
    const proxy = await fakeProxy()
    const grant: ResolvedInferenceGrant = {
      provider: "sarah_proxy",
      grantRef: InferenceGrantRef.make("grant_e2e"),
      providerAccountRef: ProviderAccountRef.make("account_e2e"),
      providerSecretRef: ProviderSecretRef.make("secret_e2e"),
      runnerSessionId: "session_e2e",
      inferenceUrl: proxy.url,
      status: "active",
      expiresAt: Number.MAX_SAFE_INTEGER,
      budget: { maxTokens: 1000, maxCalls: 4, wallClockMs: 60_000 },
      materialization: {
        providerSecretRef: ProviderSecretRef.make("secret_e2e"),
        target: { kind: "env", name: "PROBE_INFERENCE_GRANT" }
      }
    }

    const stopReason = await Effect.runPromise(
      withMaterializedGrant(
        { grant, runHome: mkdtempSync(join(tmpdir(), "probe-e2e-")), broker: literalBroker("grant_token_e2e") },
        (materialized) =>
          Effect.promise(
            () =>
              new Promise<string>((resolveReason, rejectReason) => {
                // Exactly the controller's spawn shape: scrubbed base env plus
                // the materialized grant env for the first-party agent.
                const child = spawn(PROBE_BIN, ["acp"], {
                  env: { ...process.env, PROBE_TRANSPORT: "openai", PROBE_MODEL: "test", ...materialized.env },
                  stdio: ["pipe", "pipe", "ignore"]
                })
                let buffer = ""
                const timer = setTimeout(() => {
                  child.kill()
                  rejectReason(new Error("timed out"))
                }, 15_000)
                child.stdout.on("data", (chunk) => {
                  buffer += chunk
                  let index = buffer.indexOf("\n")
                  while (index !== -1) {
                    const line = buffer.slice(0, index)
                    buffer = buffer.slice(index + 1)
                    index = buffer.indexOf("\n")
                    if (line.trim() === "") continue
                    const value = JSON.parse(line)
                    if (value.id === 1) {
                      child.stdin.write(
                        `{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}\n`
                      )
                    } else if (value.id === 2) {
                      const sessionId = value.result.sessionId
                      child.stdin.write(
                        `{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"${sessionId}","prompt":[{"type":"text","text":"ping"}]}}\n`
                      )
                    } else if (value.id === 3) {
                      clearTimeout(timer)
                      child.kill()
                      resolveReason(value.result.stopReason)
                    }
                  }
                })
                child.stdin.write(
                  `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}\n`
                )
              })
          )
      )
    )

    expect(stopReason).toBe("end_turn")
    // The grant reached the proxy as the bearer, exactly once minted.
    expect(proxy.bearer()).toBe("Bearer grant_token_e2e")
    proxy.close()
  }, 20_000)
})
