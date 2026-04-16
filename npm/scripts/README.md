# Probe npm package staging

This folder holds the checked-in npm wrapper and local package staging logic
for the mac-first `@openagentsinc/probe` install surface.

Current status:

- npm wrapper package exists
- first supported target is `aarch64-apple-darwin`
- the meta package is `@openagentsinc/probe`
- the first platform payload alias is `@openagentsinc/probe-darwin-arm64`

The builder script stages either:

- the thin meta package
- the mac payload package that contains only `vendor/`

## Local smoke test

Build the native Probe binary first:

```bash
cargo build --release -p probe-cli
```

Create a temporary vendor tree shaped like the platform package expects:

```bash
mkdir -p /tmp/probe-npm-vendor/aarch64-apple-darwin/probe
cp target/release/probe-cli /tmp/probe-npm-vendor/aarch64-apple-darwin/probe/probe
chmod 0755 /tmp/probe-npm-vendor/aarch64-apple-darwin/probe/probe
```

The current cargo package is still named `probe-cli`, so the local release
binary is copied and renamed into the vendor tree as `probe`.

Stage the meta package with a local vendor fallback for development:

```bash
./npm/scripts/build_npm_package.py \
  --package probe \
  --version 0.1.0 \
  --vendor-src /tmp/probe-npm-vendor \
  --staging-dir /tmp/probe-npm-stage-meta
```

Run the launcher directly:

```bash
node /tmp/probe-npm-stage-meta/bin/probe.js --help
```

Stage the mac payload package:

```bash
./npm/scripts/build_npm_package.py \
  --package probe-darwin-arm64 \
  --version 0.1.0 \
  --vendor-src /tmp/probe-npm-vendor \
  --staging-dir /tmp/probe-npm-stage-darwin-arm64
```

Pack either staged directory when needed:

```bash
./npm/scripts/build_npm_package.py \
  --package probe \
  --version 0.1.0 \
  --vendor-src /tmp/probe-npm-vendor \
  --pack-output /tmp/probe-npm/probe-npm-0.1.0.tgz
```

The broader release staging and publish flow now lives in the repo-root
`scripts/stage_npm_packages.py`.

## Release staging

Create a compressed native mac artifact from the current release binary:

```bash
cargo build --release -p probe-cli
gzip -c target/release/probe-cli > /tmp/probe-aarch64-apple-darwin.gz
```

Stage both the payload tarball and the meta tarball:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.1.0 \
  --artifact-path /tmp/probe-aarch64-apple-darwin.gz \
  --output-dir /tmp/probe-npm-dist
```

Publish in the correct order and with the correct tags:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.1.0 \
  --artifact-path /tmp/probe-aarch64-apple-darwin.gz \
  --output-dir /tmp/probe-npm-dist \
  --publish
```

The script publishes:

- `probe-darwin-arm64` with npm tag `darwin-arm64`
- `probe` with npm tag `latest`
