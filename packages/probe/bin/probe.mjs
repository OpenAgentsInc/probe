#!/usr/bin/env node
// The pinned-catalog entry point: `node bin/probe.mjs acp` serves ACP v1
// over stdio, the surface sarah-computer-controller spawns via
// [process.execPath, binPath] (spec Addendum A3). The controller sees the
// same wire protocol as the native probe-bin; the wasm core drives it.
import { runAcpStdio } from "../src/host.ts"

const mode = process.argv[2] ?? "acp"
if (mode !== "acp") {
  process.stderr.write(`probe: unknown mode ${JSON.stringify(mode)} (expected: acp)\n`)
  process.exit(2)
}
runAcpStdio().catch((error) => {
  process.stderr.write(`probe: ${error?.message ?? error}\n`)
  process.exit(1)
})
