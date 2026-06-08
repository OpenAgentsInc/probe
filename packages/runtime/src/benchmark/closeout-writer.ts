import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Effect, Schema as S } from "effect";
import {
  PROBE_BENCHMARK_CLOSEOUT_SCHEMA_REF,
  PROBE_BENCHMARK_DECISION_TRACE_SCHEMA_REF,
  PROBE_BENCHMARK_RUN_SCHEMA_REF,
  ProbeBenchmarkCloseout,
  ProbeBenchmarkContractError,
  ProbeBenchmarkDecisionTrace,
  ProbeBenchmarkFailureClassification,
  ProbeBenchmarkPolicyFinding,
  ProbeBenchmarkRun,
  decodeProbeBenchmarkCloseout,
  decodeProbeBenchmarkDecisionTrace,
  decodeProbeBenchmarkRun,
  sanitizeProbeBenchmarkProjection,
  validateProbeBenchmarkPublicProjection,
  type ProbeBenchmarkAssignment,
  type ProbeBenchmarkEvidenceSplit,
  type ProbeBenchmarkFailureFamily,
  type ProbeBenchmarkPromotionStatus,
  type ProbeBenchmarkRedactionState,
  type ProbeBenchmarkResourceCostRefs,
  type ProbeBenchmarkRunStatus,
} from "../contracts/benchmark";
import { type JsonValue, type ProbePublicProjectionUnsafe } from "../contracts/provider-account";

export const PROBE_BENCHMARK_CLOSEOUT_BUNDLE_SCHEMA_REF = "probe.benchmark_closeout_bundle.v1" as const;

export const PROBE_BENCHMARK_CLOSEOUT_BUNDLE_FILE_NAMES = [
  "probe-run-record.json",
  "probe-closeout.json",
  "decision-trace-summary.json",
  "selected-signatures.json",
  "tool-menu.json",
  "candidate-ref.json",
  "artifact-refs.json",
  "resource-usage-ref.json",
  "policy-findings.json",
  "failure-classification.json",
] as const;
export type ProbeBenchmarkCloseoutBundleFileName = (typeof PROBE_BENCHMARK_CLOSEOUT_BUNDLE_FILE_NAMES)[number];

export type ProbeBenchmarkTerminalRunStatus = Extract<
  ProbeBenchmarkRunStatus,
  "succeeded" | "failed" | "timed_out" | "policy_blocked" | "errored"
>;

export interface ProbeBenchmarkCloseoutWriterInput {
  readonly assignment: ProbeBenchmarkAssignment;
  readonly artifactManifestRefs?: ReadonlyArray<string>;
  readonly backendRouteRef?: string;
  readonly candidateComponentRefs?: ReadonlyArray<string>;
  readonly completedAt?: string;
  readonly costRef?: string;
  readonly decisionStepRefs?: ReadonlyArray<string>;
  readonly failureClassification?: ProbeBenchmarkFailureClassification;
  readonly observedAt?: string;
  readonly partialArtifactRefs?: ReadonlyArray<string>;
  readonly policyFindings?: ReadonlyArray<ProbeBenchmarkPolicyFinding>;
  readonly proofBundleRefs?: ReadonlyArray<string>;
  readonly redactionState?: ProbeBenchmarkRedactionState;
  readonly resourceUnavailableReason?: string;
  readonly resourceUsageRef?: string;
  readonly retainedFailureRefs?: ReadonlyArray<string>;
  readonly runRef: string;
  readonly runStatus: ProbeBenchmarkTerminalRunStatus;
  readonly scorerRef: string;
  readonly startedAt?: string;
  readonly summaryArtifactRef?: string;
  readonly toolMenuSnapshot?: JsonValue;
  readonly verifierRef: string;
  readonly verifierResultRefs?: ReadonlyArray<string>;
}

export interface ProbeBenchmarkCloseoutBundle {
  readonly assignmentRef: string;
  readonly bundleRef: string;
  readonly candidateHash: string;
  readonly evidenceSplit: ProbeBenchmarkEvidenceSplit;
  readonly files: Readonly<Record<ProbeBenchmarkCloseoutBundleFileName, JsonValue>>;
  readonly runRef: string;
  readonly schemaRef: typeof PROBE_BENCHMARK_CLOSEOUT_BUNDLE_SCHEMA_REF;
}

export interface ProbeBenchmarkCloseoutBundleWriteResult {
  readonly bundleRef: string;
  readonly directory: string;
  readonly files: ReadonlyArray<{
    readonly fileName: ProbeBenchmarkCloseoutBundleFileName;
    readonly path: string;
  }>;
}

export class ProbeBenchmarkCloseoutWriterError extends S.TaggedErrorClass<ProbeBenchmarkCloseoutWriterError>()(
  "ProbeBenchmarkCloseoutWriterError",
  {
    path: S.String,
    reason: S.String,
  },
) {}

export function makeProbeBenchmarkCloseoutBundle(
  input: ProbeBenchmarkCloseoutWriterInput,
): Effect.Effect<
  ProbeBenchmarkCloseoutBundle,
  ProbeBenchmarkCloseoutWriterError | ProbeBenchmarkContractError | ProbePublicProjectionUnsafe
> {
  return Effect.gen(function* () {
    yield* validateProbeBenchmarkPublicProjection(input, "benchmarkCloseoutWriterInput");

    const assignment = input.assignment;
    const observedAt = input.observedAt ?? new Date().toISOString();
    const artifactManifestRefs = materialRefs(
      input.artifactManifestRefs,
      assignment.requiredArtifacts.artifactRefs,
    );
    const proofBundleRefs = materialRefs(input.proofBundleRefs, assignment.requiredArtifacts.proofBundleRefs);
    const partialArtifactRefs = [...(input.partialArtifactRefs ?? [])];
    const resourceCostRefs = makeResourceCostRefs(input);
    const failureClassification = input.failureClassification ?? defaultFailureClassification(input.runStatus);
    const retainedFailureRefs = retainedRefsFor(input, failureClassification.family);
    const policyFindings = policyFindingsFor(input);
    const promotionStatus = input.runStatus === "succeeded"
      ? promotionStatusForSplit(assignment.split.evidenceSplit)
      : "blocked";
    const summaryArtifactRef = input.summaryArtifactRef ?? `artifact.probe.benchmark.${input.runRef}.decision_trace_summary`;

    if (resourceCostRefs.resourceUsageRef === undefined && resourceCostRefs.unavailableReason === undefined) {
      return yield* Effect.fail(
        new ProbeBenchmarkCloseoutWriterError({
          path: "benchmarkCloseoutWriterInput.resourceUsageRef",
          reason: "must include resourceUsageRef or resourceUnavailableReason",
        }),
      );
    }

    const runRecord = yield* decodeProbeBenchmarkRun({
      schemaRef: PROBE_BENCHMARK_RUN_SCHEMA_REF,
      assignmentRef: assignment.assignmentRef,
      candidateHash: assignment.candidateHash,
      closeoutRef: closeoutRefFor(input.runRef),
      completedAt: input.completedAt ?? observedAt,
      evidenceSplit: assignment.split.evidenceSplit,
      resultSummaryRef: summaryArtifactRef,
      runRef: input.runRef,
      startedAt: input.startedAt ?? observedAt,
      status: input.runStatus,
    });

    const decisionTrace = yield* decodeProbeBenchmarkDecisionTrace({
      schemaRef: PROBE_BENCHMARK_DECISION_TRACE_SCHEMA_REF,
      assignmentRef: assignment.assignmentRef,
      candidateHash: assignment.candidateHash,
      decisionStepRefs: input.decisionStepRefs ?? [],
      redactionState: input.redactionState ?? "public_safe",
      runRef: input.runRef,
      selectedSignatureRefs: assignment.selectedBlueprintSignatureRefs,
      summaryArtifactRef,
      toolMenuRef: assignment.toolMenuRef,
      traceRef: `decision_trace.probe.benchmark.${input.runRef}`,
    });

    const closeout = yield* decodeProbeBenchmarkCloseout({
      schemaRef: PROBE_BENCHMARK_CLOSEOUT_SCHEMA_REF,
      artifactManifestRefs,
      assignmentRef: assignment.assignmentRef,
      backendRoute: {
        backendRef: assignment.backend.backendRef,
        backendRouteRef: input.backendRouteRef ?? `backend_route.${assignment.backend.backendRef}.${assignment.runtime.backendProfileRef}`,
        modelBackendRef: assignment.backend.modelBackendRef,
        runtimeProfileRef: assignment.runtime.backendProfileRef,
      },
      candidateHash: assignment.candidateHash,
      closeoutRef: closeoutRefFor(input.runRef),
      evidenceSplit: assignment.split.evidenceSplit,
      failureClassification,
      policyFindings,
      promotionStatus,
      proofBundleRefs,
      redactionState: input.redactionState ?? "public_safe",
      resourceCostRefs,
      retainedFailureRefs,
      runRef: input.runRef,
      runStatus: input.runStatus,
      selectedSignatureRefs: assignment.selectedBlueprintSignatureRefs,
      toolMenuRef: assignment.toolMenuRef,
      verifierScorerRefs: {
        scorerRef: input.scorerRef,
        verifierRef: input.verifierRef,
      },
    });

    const files: Record<ProbeBenchmarkCloseoutBundleFileName, JsonValue> = {
      "probe-run-record.json": toJsonValue(runRecord),
      "probe-closeout.json": toJsonValue(closeout),
      "decision-trace-summary.json": toJsonValue(decisionTraceSummary(decisionTrace)),
      "selected-signatures.json": toJsonValue({
        schemaRef: "probe.selected_signatures_summary.v1",
        assignmentRef: assignment.assignmentRef,
        registrySplitRef: assignment.split.splitRef,
        selectedSignatureRefs: assignment.selectedBlueprintSignatureRefs,
      }),
      "tool-menu.json": toJsonValue({
        schemaRef: "probe.tool_menu_summary.v1",
        assignmentRef: assignment.assignmentRef,
        redactionState: input.redactionState ?? "public_safe",
        selectedSignatureRefs: assignment.selectedBlueprintSignatureRefs,
        snapshot: input.toolMenuSnapshot === undefined ? undefined : sanitizeProbeBenchmarkProjection(input.toolMenuSnapshot),
        toolMenuRef: assignment.toolMenuRef,
      }),
      "candidate-ref.json": toJsonValue({
        schemaRef: "probe.candidate_ref_summary.v1",
        assignmentRef: assignment.assignmentRef,
        candidateHash: assignment.candidateHash,
        candidateComponentRefs: [...(input.candidateComponentRefs ?? [])],
        candidateRefs: assignment.candidateRefs ?? {},
        evidenceSplit: assignment.split.evidenceSplit,
      }),
      "artifact-refs.json": toJsonValue({
        schemaRef: "probe.artifact_refs_summary.v1",
        assignmentRef: assignment.assignmentRef,
        artifactManifestRefs,
        partialArtifactRefs,
        proofBundleRefs,
        runStatus: input.runStatus,
        verifierResultRefs: [...(input.verifierResultRefs ?? [])],
      }),
      "resource-usage-ref.json": toJsonValue({
        schemaRef: "probe.resource_usage_ref_summary.v1",
        assignmentRef: assignment.assignmentRef,
        ...resourceCostRefs,
      }),
      "policy-findings.json": toJsonValue({
        schemaRef: "probe.policy_findings_summary.v1",
        assignmentRef: assignment.assignmentRef,
        policyFindings,
      }),
      "failure-classification.json": toJsonValue({
        schemaRef: "probe.failure_classification_summary.v1",
        assignmentRef: assignment.assignmentRef,
        failureClassification,
        retainedFailureRefs,
      }),
    };

    return {
      assignmentRef: assignment.assignmentRef,
      bundleRef: `probe_benchmark_closeout_bundle.${input.runRef}`,
      candidateHash: assignment.candidateHash,
      evidenceSplit: assignment.split.evidenceSplit,
      files,
      runRef: input.runRef,
      schemaRef: PROBE_BENCHMARK_CLOSEOUT_BUNDLE_SCHEMA_REF,
    };
  });
}

export function writeProbeBenchmarkCloseoutBundle(
  bundle: ProbeBenchmarkCloseoutBundle,
  directory: string,
): Effect.Effect<ProbeBenchmarkCloseoutBundleWriteResult, ProbeBenchmarkCloseoutWriterError> {
  return Effect.tryPromise({
    try: async () => {
      await mkdir(directory, { recursive: true });

      const files = await Promise.all(
        PROBE_BENCHMARK_CLOSEOUT_BUNDLE_FILE_NAMES.map(async (fileName) => {
          const path = join(directory, fileName);
          await writeFile(path, `${JSON.stringify(bundle.files[fileName], null, 2)}\n`, "utf8");
          return { fileName, path };
        }),
      );

      return {
        bundleRef: bundle.bundleRef,
        directory,
        files,
      };
    },
    catch: (error) =>
      new ProbeBenchmarkCloseoutWriterError({
        path: directory,
        reason: error instanceof Error ? error.message : String(error),
      }),
  });
}

function materialRefs(
  explicitRefs: ReadonlyArray<string> | undefined,
  requiredRefs: ReadonlyArray<string>,
): ReadonlyArray<string> {
  return explicitRefs === undefined ? [...requiredRefs] : [...explicitRefs];
}

function makeResourceCostRefs(input: ProbeBenchmarkCloseoutWriterInput): ProbeBenchmarkResourceCostRefs {
  if (input.resourceUsageRef !== undefined) {
    return {
      costRef: input.costRef,
      resourceUsageRef: input.resourceUsageRef,
    };
  }

  return {
    costRef: input.costRef,
    unavailableReason: input.resourceUnavailableReason ?? `${input.runStatus}_resource_usage_unavailable`,
  };
}

function defaultFailureClassification(runStatus: ProbeBenchmarkTerminalRunStatus): ProbeBenchmarkFailureClassification {
  return {
    classificationRef: `failure_classification.probe.${runStatus}`,
    family: failureFamilyForStatus(runStatus),
  };
}

function failureFamilyForStatus(runStatus: ProbeBenchmarkTerminalRunStatus): ProbeBenchmarkFailureFamily {
  switch (runStatus) {
    case "succeeded":
      return "none";
    case "timed_out":
      return "timeout";
    case "policy_blocked":
      return "policy_blocked";
    case "errored":
      return "runtime_error";
    case "failed":
      return "unknown";
  }
}

function retainedRefsFor(
  input: ProbeBenchmarkCloseoutWriterInput,
  failureFamily: ProbeBenchmarkFailureFamily,
): ReadonlyArray<string> {
  if (input.retainedFailureRefs !== undefined) {
    return [...input.retainedFailureRefs];
  }

  if (input.assignment.split.evidenceSplit !== "retained" || input.runStatus === "succeeded") {
    return [];
  }

  return [`retained_failure.${input.assignment.dataset.slug}.${input.assignment.taskRunRef}.${failureFamily}`];
}

function policyFindingsFor(input: ProbeBenchmarkCloseoutWriterInput): ReadonlyArray<ProbeBenchmarkPolicyFinding> {
  if (input.policyFindings !== undefined) {
    return [...input.policyFindings];
  }

  return input.runStatus === "policy_blocked"
    ? [
        {
          findingRef: `policy_finding.probe.benchmark.${input.runRef}.blocked`,
          severity: "blocked",
        },
      ]
    : [];
}

function promotionStatusForSplit(evidenceSplit: ProbeBenchmarkEvidenceSplit): ProbeBenchmarkPromotionStatus {
  switch (evidenceSplit) {
    case "retained":
      return "retained_evidence";
    case "validation":
      return "validation_candidate";
    case "holdout":
      return "holdout_candidate";
    case "live":
      return "live_evidence";
  }
}

function closeoutRefFor(runRef: string): string {
  return `probe_closeout.${runRef}`;
}

function decisionTraceSummary(trace: ProbeBenchmarkDecisionTrace): JsonValue {
  return {
    schemaRef: "probe.decision_trace_summary.v1",
    assignmentRef: trace.assignmentRef,
    candidateHash: trace.candidateHash,
    decisionStepRefs: trace.decisionStepRefs,
    redactionState: trace.redactionState,
    runRef: trace.runRef,
    selectedSignatureRefs: trace.selectedSignatureRefs,
    summaryArtifactRef: trace.summaryArtifactRef,
    toolMenuRef: trace.toolMenuRef,
    traceRef: trace.traceRef,
  };
}

function toJsonValue(value: unknown): JsonValue {
  return JSON.parse(JSON.stringify(value)) as JsonValue;
}
