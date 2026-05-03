FROM rust:1.86-bookworm AS build

WORKDIR /app
COPY . .
RUN cargo build --release -p probe-cli

FROM gcr.io/google.com/cloudsdktool/google-cloud-cli:slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git openssh-client \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/probe-cli /usr/local/bin/probe
COPY scripts/deploy/managed-cloud-run-worker-pool/probe-managed-cloud-run-worker-pool.sh /usr/local/bin/probe-managed-cloud-run-worker-pool

ENV PROBE_HOME=/var/lib/probe
ENV PROBE_MANAGED_CWD=/workspace
ENV PROBE_MANAGED_WORKER_SHUTDOWN_FILE=/tmp/probe-managed-worker-pool.shutdown

ENTRYPOINT ["/usr/local/bin/probe-managed-cloud-run-worker-pool"]
