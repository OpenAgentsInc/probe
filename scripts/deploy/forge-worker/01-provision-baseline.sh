#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

require_cmd gcloud

ensure_gcloud_context
ensure_services

if ! gcloud iam service-accounts describe "$PROBE_FORGE_SERVICE_ACCOUNT_EMAIL" \
  --project "$GCP_PROJECT" >/dev/null 2>&1; then
  log "Creating service account: ${PROBE_FORGE_SERVICE_ACCOUNT_NAME}"
  gcloud iam service-accounts create "$PROBE_FORGE_SERVICE_ACCOUNT_NAME" \
    --project "$GCP_PROJECT" \
    --display-name "Probe Forge worker" >/dev/null
fi

for role in \
  roles/logging.logWriter \
  roles/monitoring.metricWriter; do
  log "Ensuring IAM binding: ${role}"
  gcloud projects add-iam-policy-binding "$GCP_PROJECT" \
    --member "serviceAccount:${PROBE_FORGE_SERVICE_ACCOUNT_EMAIL}" \
    --role "$role" >/dev/null
done

if ! disk_exists "$PROBE_FORGE_DATA_DISK"; then
  log "Creating disk: ${PROBE_FORGE_DATA_DISK}"
  gcloud compute disks create "$PROBE_FORGE_DATA_DISK" \
    --project "$GCP_PROJECT" \
    --zone "$GCP_ZONE" \
    --size "${PROBE_FORGE_DATA_DISK_SIZE_GB}GB" \
    --type "$PROBE_FORGE_DATA_DISK_TYPE" >/dev/null
fi

if ! instance_exists "$PROBE_FORGE_VM"; then
  log "Creating VM: ${PROBE_FORGE_VM}"
  gcloud compute instances create "$PROBE_FORGE_VM" \
    --project "$GCP_PROJECT" \
    --zone "$GCP_ZONE" \
    --machine-type "$PROBE_FORGE_MACHINE_TYPE" \
    --image-family ubuntu-2204-lts \
    --image-project ubuntu-os-cloud \
    --boot-disk-size 100GB \
    --boot-disk-type pd-ssd \
    --network-interface "subnet=${OA_SUBNET},no-address" \
    --service-account "$PROBE_FORGE_SERVICE_ACCOUNT_EMAIL" \
    --scopes cloud-platform \
    --disk "name=${PROBE_FORGE_DATA_DISK},device-name=${PROBE_FORGE_DATA_DISK_DEVICE_NAME},mode=rw,auto-delete=no" \
    --tags "$PROBE_FORGE_TAG" \
    --metadata "enable-oslogin=TRUE" >/dev/null
else
  log "VM already exists: ${PROBE_FORGE_VM}"
fi

ATTACHED_DISK_SOURCE="$(gcloud compute instances describe "$PROBE_FORGE_VM" \
  --project "$GCP_PROJECT" \
  --zone "$GCP_ZONE" \
  --format='value(disks[].source)' | grep "/disks/${PROBE_FORGE_DATA_DISK}$" || true)"

if [[ -z "$ATTACHED_DISK_SOURCE" ]]; then
  log "Attaching missing disk ${PROBE_FORGE_DATA_DISK} to ${PROBE_FORGE_VM}"
  gcloud compute instances attach-disk "$PROBE_FORGE_VM" \
    --project "$GCP_PROJECT" \
    --zone "$GCP_ZONE" \
    --disk "$PROBE_FORGE_DATA_DISK" \
    --device-name "$PROBE_FORGE_DATA_DISK_DEVICE_NAME" >/dev/null
fi

if firewall_rule_exists "oa-allow-probe-forge-worker-iap-ssh"; then
  log "Updating firewall rule: oa-allow-probe-forge-worker-iap-ssh"
  gcloud compute firewall-rules update "oa-allow-probe-forge-worker-iap-ssh" \
    --project "$GCP_PROJECT" \
    --allow tcp:22 \
    --target-tags "$PROBE_FORGE_TAG" \
    --source-ranges "35.235.240.0/20" >/dev/null
else
  log "Creating firewall rule: oa-allow-probe-forge-worker-iap-ssh"
  gcloud compute firewall-rules create "oa-allow-probe-forge-worker-iap-ssh" \
    --project "$GCP_PROJECT" \
    --network "$OA_VPC" \
    --allow tcp:22 \
    --target-tags "$PROBE_FORGE_TAG" \
    --source-ranges "35.235.240.0/20" >/dev/null
fi

log "Provisioning complete"
log "vm=${PROBE_FORGE_VM}"
log "probe_home=${PROBE_FORGE_PROBE_HOME}"
log "service_name=${PROBE_FORGE_SERVICE_NAME}"

