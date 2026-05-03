FROM rust:1.86-bookworm AS build

WORKDIR /app
COPY . .
RUN cargo build --release -p probe-cli

FROM gcr.io/google.com/cloudsdktool/google-cloud-cli:slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends bash ca-certificates curl git openssh-client \
  && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/probe-cli /usr/local/bin/probe
COPY scripts/deploy/managed-cloud-run-job/probe-managed-cloud-run-job.sh /usr/local/bin/probe-managed-cloud-run-job

ENV PROBE_HOME=/var/lib/probe
ENV PROBE_MANAGED_CWD=/workspace

RUN mkdir -p /var/lib/probe /workspace \
  && chmod +x /usr/local/bin/probe /usr/local/bin/probe-managed-cloud-run-job

ENTRYPOINT ["/usr/local/bin/probe-managed-cloud-run-job"]
