# Mac-First npm Release Staging

This document covers the first real release staging layer above the checked-in
Probe npm wrapper.

Current scope remains intentionally narrow:

- one supported native target: `aarch64-apple-darwin`
- one supported payload alias: `@openagentsinc/probe-darwin-arm64`
- one public install target: `@openagentsinc/probe`

## Why there are two published versions of the same package

The package model mirrors the Codex approach:

- publish one platform payload version under the same underlying npm package
  name
- publish the thin meta package version separately
- keep `latest` on the thin meta package so `npm i -g @openagentsinc/probe`
  resolves to the launcher package, not the platform payload

For version `0.1.0`, that means:

- payload package version: `0.1.0-darwin-arm64`
- meta package version: `0.1.0`

The meta package points at the payload package through:

```json
{
  "optionalDependencies": {
    "@openagentsinc/probe-darwin-arm64": "npm:@openagentsinc/probe@0.1.0-darwin-arm64"
  }
}
```

## Dist-tag rule

The publish tags must be split:

- payload package: publish with tag `darwin-arm64`
- meta package: publish with tag `latest`

Do not publish the payload package under `latest`.

If the payload package claims `latest`, a plain `npm install @openagentsinc/probe`
can resolve to the platform payload version instead of the thin meta package,
which breaks the install model.

## Native artifact shape

The checked-in installer now accepts a compressed native artifact and writes it
into the expected vendor layout:

- installer: `npm/scripts/install_native_deps.py`
- destination layout:

```text
vendor/
  aarch64-apple-darwin/
    probe/
      probe
```

The simplest local mac artifact is a gzip-compressed release binary:

```bash
cargo build --release -p probe-cli
gzip -c target/release/probe-cli > /tmp/probe-aarch64-apple-darwin.gz
```

That artifact is then installed into a temporary vendor tree before npm
tarballs are built.

## Repo-level staging script

The repo-level entry point is:

- `scripts/stage_npm_packages.py`

It:

1. installs the native mac artifact into a temporary `vendor/` tree
2. stages the mac payload package tarball
3. stages the thin meta package tarball
4. optionally publishes both tarballs to npm in the correct order

## Local staging command

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.1.0 \
  --artifact-path /tmp/probe-aarch64-apple-darwin.gz \
  --output-dir /tmp/probe-npm-dist
```

Expected outputs:

- `/tmp/probe-npm-dist/probe-npm-darwin-arm64-0.1.0.tgz`
- `/tmp/probe-npm-dist/probe-npm-0.1.0.tgz`

## Publish command

To publish with the checked-in script:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.1.0 \
  --artifact-path /tmp/probe-aarch64-apple-darwin.gz \
  --output-dir /tmp/probe-npm-dist \
  --publish
```

That publishes in this order:

1. payload tarball with tag `darwin-arm64`
2. meta tarball with tag `latest`

The payload tarball deliberately keeps the canonical package name
`@openagentsinc/probe` and relies on npm aliasing from the meta package's
`optionalDependencies`, matching the `@openai/codex` pattern. Directly
installing both tarballs side-by-side from local files is not a faithful
simulation of the real registry install path because npm does not recreate that
alias resolution flow in the same way.

If npm publish requires 2FA, pass the OTP directly:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.1.0 \
  --artifact-path /tmp/probe-aarch64-apple-darwin.gz \
  --output-dir /tmp/probe-npm-dist \
  --publish \
  --otp 123456
```

## Required smoke test after publish

After publishing, the honest mac smoke test is:

```bash
npm i -g @openagentsinc/probe
probe
probe exec --profile openai-codex-subscription "hello"
```

`probe` should open the TUI by default. The `exec` example above assumes this
machine already has a working hosted Codex login saved under `PROBE_HOME`; if
it does not, use another configured backend profile for the second smoke step.

Before publish, the honest local proof points are narrower:

- `./scripts/stage_npm_packages.py --release-version ... --artifact-path ...`
  should produce both tarballs
- `node <staged-meta-dir>/bin/probe.js --help` should launch the staged native
  payload when `vendor/` is present
- the published-registry install must still be run afterward to prove the alias
  flow used by `npm i -g @openagentsinc/probe`

That smoke test belongs above the release staging layer because it validates
the real registry path, not just local tarball construction.
