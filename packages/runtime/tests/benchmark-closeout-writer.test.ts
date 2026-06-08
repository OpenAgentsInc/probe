import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "bun:test";
import { Effect } from "effect";
import {
  PROBE_BENCHMARK_ASSIGNMENT_SCHEMA_REF,
  PROBE_BENCHMARK_CLOSEOUT_BUNDLE_FILE_NAMES,
  decodeProbeBenchmarkAssignment,
  makeProbeBenchmarkCloseoutBundle,
  writeProbeBenchmarkCloseoutBundle,
} from "../src";

const fakeAssignment = async () =>
  Effect.runPromise(
    decodeProbeBenchmarkAssignment({
      schemaRef: PROBE_BENCHMARK_ASSIGNMENT_SCHEMA_REF,
      assignmentRef: "probe_benchmark_assignment.configure_git_webserver.1",
      benchmarkRunRef: "benchmark_run.terminal_bench_2.gepa_stage_0.1",
      taskRunRef: "task_run.configure_git_webserver.1",
      dataset: {
        slug: "terminal-bench-2-harbor",
        version: "2026-06-08",
      },
      split: {
        evidenceSplit: "retained",
        splitRef: "split.terminal_bench_2.retained.v1",
      },
      task: {
        taskChecksum: "sha256:9d7a6f8f1b7d0f5e0f0d4c8e2a4f7b3e",
      },
      probeCommit: "abc1234",
      runtime: {
        runtimeRef: "runtime.probe.v1",
        backendProfileRef: "backend_profile.apple_fm.local.v1",
      },
      backend: {
        backendRef: "probe.backend.apple_fm_bridge",
        modelBackendRef: "model_backend.apple_fm.local_foundation_model",
      },
      selectedBlueprintSignatureRefs: ["program_signature.probe.benchmark.service_readiness.v1"],
      toolMenuRef: "tool_menu.probe.terminal_bench.service_readiness.v1",
      candidateHash: "sha256:candidate-1",
      candidateRefs: {
        promptCandidateRef: "candidate.prompt.service_readiness.v1",
        blueprintCandidateRef: "candidate.blueprint.service_readiness.v1",
        toolMenuCandidateRef: "candidate.tool_menu.service_readiness.v1",
        loopPolicyCandidateRef: "candidate.loop_policy.service_readiness.v1",
      },
      timeoutBudgetPolicy: {
        budgetPolicyRef: "budget_policy.probe.retained_smoke.v1",
        timeoutPolicyRef: "timeout_policy.probe.retained_smoke.v1",
      },
      requiredArtifacts: {
        artifactRefs: ["artifact_manifest.required.probe.closeout.v1"],
        proofBundleRefs: ["proof_bundle.required.probe.closeout.v1"],
      },
      sinks: {
        callbackRefs: ["callback.openagents.benchmark_cloud.probe.v1"],
        proofSinkRefs: ["proof_sink.openagents.benchmark_cloud.probe.v1"],
      },
    }),
  );

describe("Probe benchmark closeout writer", () => {
  test("emits and writes a complete successful closeout bundle", async () => {
    const assignment = await fakeAssignment();
    const bundle = await Effect.runPromise(
      makeProbeBenchmarkCloseoutBundle({
        assignment,
        artifactManifestRefs: ["artifact_manifest.probe.configure_git_webserver.1"],
        decisionStepRefs: ["decision_step.inspect_service_status.1"],
        proofBundleRefs: ["proof_bundle.probe.configure_git_webserver.1"],
        resourceUsageRef: "resource_usage.probe.configure_git_webserver.1",
        runRef: "probe_run.configure_git_webserver.1",
        runStatus: "succeeded",
        scorerRef: "scorer.terminal_bench.binary.v1",
        toolMenuSnapshot: {
          toolRefs: ["tool.probe.read_file", "tool.probe.code_search"],
        },
        verifierRef: "verifier.terminal_bench.configure_git_webserver.v1",
      }),
    );
    const directory = await mkdtemp(join(tmpdir(), "probe-closeout-"));

    try {
      const writeResult = await Effect.runPromise(writeProbeBenchmarkCloseoutBundle(bundle, directory));
      const closeoutFile = JSON.parse(await readFile(join(directory, "probe-closeout.json"), "utf8"));

      expect(Object.keys(bundle.files).sort()).toEqual([...PROBE_BENCHMARK_CLOSEOUT_BUNDLE_FILE_NAMES].sort());
      expect(writeResult.files.map((file) => file.fileName).sort()).toEqual(
        [...PROBE_BENCHMARK_CLOSEOUT_BUNDLE_FILE_NAMES].sort(),
      );
      expect(closeoutFile.runStatus).toBe("succeeded");
      expect(closeoutFile.artifactManifestRefs).toEqual(["artifact_manifest.probe.configure_git_webserver.1"]);
      expect(JSON.stringify(bundle.files)).not.toContain("raw");
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });

  test("failed retained runs emit retained-failure refs and failure classification", async () => {
    const assignment = await fakeAssignment();
    const bundle = await Effect.runPromise(
      makeProbeBenchmarkCloseoutBundle({
        assignment,
        failureClassification: {
          classificationRef: "failure_classification.configure_git_webserver.service_readiness",
          family: "service_readiness",
          summaryRef: "summary.failure.configure_git_webserver.1",
        },
        resourceUsageRef: "resource_usage.probe.configure_git_webserver.failed.1",
        runRef: "probe_run.configure_git_webserver.failed.1",
        runStatus: "failed",
        scorerRef: "scorer.terminal_bench.binary.v1",
        verifierRef: "verifier.terminal_bench.configure_git_webserver.v1",
      }),
    );
    const closeout = bundle.files["probe-closeout.json"] as { readonly [key: string]: unknown };
    const failure = bundle.files["failure-classification.json"] as { readonly [key: string]: unknown };

    expect(closeout.runStatus).toBe("failed");
    expect((closeout.retainedFailureRefs as string[])[0]).toContain("service_readiness");
    expect(JSON.stringify(failure)).toContain("service_readiness");
  });

  test("timed-out runs emit timeout state, partial artifact refs, and resource unavailable reason", async () => {
    const assignment = await fakeAssignment();
    const bundle = await Effect.runPromise(
      makeProbeBenchmarkCloseoutBundle({
        assignment,
        artifactManifestRefs: ["artifact_manifest.partial.configure_git_webserver.timeout.1"],
        partialArtifactRefs: ["artifact.partial.stdout_summary.configure_git_webserver.timeout.1"],
        resourceUnavailableReason: "timeout_before_resource_meter_flush",
        runRef: "probe_run.configure_git_webserver.timeout.1",
        runStatus: "timed_out",
        scorerRef: "scorer.terminal_bench.binary.v1",
        verifierRef: "verifier.terminal_bench.configure_git_webserver.v1",
      }),
    );
    const closeout = bundle.files["probe-closeout.json"] as { readonly [key: string]: unknown };
    const artifacts = bundle.files["artifact-refs.json"] as { readonly [key: string]: unknown };
    const resource = bundle.files["resource-usage-ref.json"] as { readonly [key: string]: unknown };

    expect(closeout.runStatus).toBe("timed_out");
    expect((closeout.failureClassification as { readonly family: string }).family).toBe("timeout");
    expect(artifacts.partialArtifactRefs).toEqual(["artifact.partial.stdout_summary.configure_git_webserver.timeout.1"]);
    expect(resource.unavailableReason).toBe("timeout_before_resource_meter_flush");
  });

  test("policy-blocked runs emit blocked policy findings", async () => {
    const assignment = await fakeAssignment();
    const bundle = await Effect.runPromise(
      makeProbeBenchmarkCloseoutBundle({
        assignment,
        resourceUnavailableReason: "policy_blocked_before_resource_meter",
        runRef: "probe_run.configure_git_webserver.policy_blocked.1",
        runStatus: "policy_blocked",
        scorerRef: "scorer.terminal_bench.binary.v1",
        verifierRef: "verifier.terminal_bench.configure_git_webserver.v1",
      }),
    );
    const policy = bundle.files["policy-findings.json"] as { readonly [key: string]: unknown };

    expect(JSON.stringify(policy)).toContain("blocked");
  });

  test("rejects unsafe writer input before public-safe artifacts are emitted", async () => {
    const assignment = await fakeAssignment();

    await expect(
      Effect.runPromise(
        makeProbeBenchmarkCloseoutBundle({
          assignment,
          resourceUsageRef: "resource_usage.probe.configure_git_webserver.1",
          runRef: "probe_run.configure_git_webserver.unsafe.1",
          runStatus: "succeeded",
          scorerRef: "scorer.terminal_bench.binary.v1",
          toolMenuSnapshot: {
            rawLogs: "captured terminal transcript",
          },
          verifierRef: "verifier.terminal_bench.configure_git_webserver.v1",
        }),
      ),
    ).rejects.toMatchObject({
      _tag: "ProbeBenchmarkContractError",
    });
  });
});
