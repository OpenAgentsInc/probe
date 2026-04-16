#!/usr/bin/env node
// Unified entry point for the Probe CLI npm wrapper.

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const require = createRequire(import.meta.url);

const TARGET_TRIPLE = "aarch64-apple-darwin";
const PLATFORM_PACKAGE = "@openagentsinc/probe-darwin-arm64";

if (process.platform !== "darwin" || process.arch !== "arm64") {
  throw new Error(
    `Unsupported platform for the current mac-first Probe npm package: ${process.platform} (${process.arch}). Supported: darwin (arm64).`,
  );
}

const probeBinaryName = "probe";
const localVendorRoot = path.join(__dirname, "..", "vendor");
const localBinaryPath = path.join(
  localVendorRoot,
  TARGET_TRIPLE,
  "probe",
  probeBinaryName,
);

let vendorRoot;
try {
  const packageJsonPath = require.resolve(`${PLATFORM_PACKAGE}/package.json`);
  vendorRoot = path.join(path.dirname(packageJsonPath), "vendor");
} catch {
  if (existsSync(localBinaryPath)) {
    vendorRoot = localVendorRoot;
  } else {
    throw new Error(
      `Missing optional dependency ${PLATFORM_PACKAGE}. Reinstall Probe: ${reinstallCommand()}`,
    );
  }
}

const binaryPath = path.join(vendorRoot, TARGET_TRIPLE, "probe", probeBinaryName);
if (!existsSync(binaryPath)) {
  throw new Error(
    `Probe binary missing at ${binaryPath}. Reinstall Probe: ${reinstallCommand()}`,
  );
}

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
  env: { ...process.env },
});

child.on("error", (error) => {
  console.error(error);
  process.exit(1);
});

const forwardSignal = (signal) => {
  if (child.killed) {
    return;
  }
  try {
    child.kill(signal);
  } catch {
    // Ignore errors while forwarding termination signals.
  }
};

["SIGINT", "SIGTERM", "SIGHUP"].forEach((signal) => {
  process.on(signal, () => forwardSignal(signal));
});

const childResult = await new Promise((resolve) => {
  child.on("exit", (code, signal) => {
    if (signal) {
      resolve({ type: "signal", signal });
    } else {
      resolve({ type: "code", exitCode: code ?? 1 });
    }
  });
});

if (childResult.type === "signal") {
  process.kill(process.pid, childResult.signal);
} else {
  process.exit(childResult.exitCode);
}

function reinstallCommand() {
  const packageManager = detectPackageManager();
  if (packageManager === "bun") {
    return "bun install -g @openagentsinc/probe@latest";
  }
  return "npm install -g @openagentsinc/probe@latest";
}

function detectPackageManager() {
  const userAgent = process.env.npm_config_user_agent || "";
  if (/\bbun\//.test(userAgent)) {
    return "bun";
  }

  const execPath = process.env.npm_execpath || "";
  if (execPath.includes("bun")) {
    return "bun";
  }

  if (
    __dirname.includes(".bun/install/global") ||
    __dirname.includes(".bun\\install\\global")
  ) {
    return "bun";
  }

  return userAgent ? "npm" : null;
}
