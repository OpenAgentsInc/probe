# Mac-First npm Global Install Packaging

Probe now carries the first checked-in npm wrapper surface for a Codex-style
global install.

Current scope is intentionally narrow:

- public install target: `@openagentsinc/probe`
- first supported platform: macOS Apple Silicon
- first target triple: `aarch64-apple-darwin`

This is not the full release story yet. It is the package and launcher shape
needed before the release and publish flow can be layered on top.

## Package model

The npm surface follows the same basic model as the Codex repo:

- one thin npm meta package
- one platform-native payload package
- one JS launcher that selects the correct native payload and spawns it

For the mac-first Probe pass, those packages are:

- meta package alias: `@openagentsinc/probe`
- platform payload alias: `@openagentsinc/probe-darwin-arm64`

The launcher itself lives at:

- `npm/bin/probe.js`

The local npm wrapper metadata lives at:

- `npm/package.json`

## Launcher behavior

`npm/bin/probe.js` is deliberately small.

It:

1. checks that the current machine is `darwin` + `arm64`
2. maps that machine to `aarch64-apple-darwin`
3. resolves the installed platform package alias
4. finds `vendor/aarch64-apple-darwin/probe/probe`
5. spawns the native binary with inherited stdio
6. forwards common signals to the child process

For local development it also supports a `vendor/` tree directly under the
staged meta package directory. That gives us a zero-registry smoke path:

- stage a meta package with local `vendor/`
- run `node bin/probe.js --help`

The packed release tarball for the meta package still excludes `vendor/`.

## Local staging

The checked-in builder is:

- `npm/scripts/build_npm_package.py`

It currently stages:

- `probe`
- `probe-darwin-arm64`

The builder expects `--vendor-src` to point at a tree shaped like:

```text
vendor/
  aarch64-apple-darwin/
    probe/
      probe
```

Today that means the staging flow copies the Rust release artifact currently
produced at `target/release/probe-cli` into `vendor/.../probe/probe`.

The meta package stage intentionally uses:

- `files: ["bin"]`

The platform payload stage intentionally uses:

- `files: ["vendor"]`

That preserves the right split:

- the public package stays thin
- the native payload package carries the binary

## Why keep the wrapper in-repo

The npm wrapper belongs in `probe`, not in the workspace repo, because:

- the `probe` CLI binary is owned here
- the package name `probe` should track the real runtime CLI surface
- the packaging logic needs to evolve alongside the native binary and release
  shape

The next layer above this wrapper surface is now documented in
`docs/83-mac-first-npm-release-staging.md`.
