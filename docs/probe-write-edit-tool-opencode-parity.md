# Probe Write/Edit Tool Parity With OpenCode

Reference: `projects/repos/opencode` (the upstream opencode repo).
Audience: Probe implementors bringing probe's `write_file` and future `edit` tool
to 100% feature parity with opencode V2 (core) and V1 (opencode package) layers.

---

## Current Probe State

Probe has exactly one file-writing tool: `write_file` at
`packages/runtime/src/cli.ts:1073-1091` (definition) and `1197-1239` (handler).

### Parameters

| Param     | Type   | Required | Description                              |
|-----------|--------|----------|------------------------------------------|
| `path`    | string | yes      | A relative file path under the workspace |
| `content` | string | yes      | The full file content to write           |

### What it does today

1. Resolve workspace root via `PROBE_WORKSPACE_ROOT` or `OPENAGENTS_WORKSPACE_ROOT`
   env vars.
2. Resolve the requested path against the workspace; reject if the path is empty,
   contains null bytes, `..`-escapes the workspace, or enters `.git`.
3. Create parent directories with `mkdir(dirname(resolved), { recursive: true })`.
4. Write the full file content as UTF-8 via `writeFile(resolved, content, "utf8")`.
5. Return `{ path, content: "written to ..." }` on success or `{ path, error }`
   on failure.

### What it does NOT do

- No `edit` tool (partial replace via oldString/newString).
- No permission/approval gating.
- No diff preview.
- No BOM detection or preservation.
- No line-ending normalization (CRLF/LF).
- No stale-content guard (race detection).
- No per-file locking / mutex for concurrent writes.
- No LSP diagnostics after write.
- No auto-format after write.
- No event publishing for watchers.
- No snapshot / undo metadata.

---

## Target: OpenCode V2 Tools

OpenCode V2 lives in `packages/core/src/tool/`. It has two file-mutation tools:

### V2 `write` (`packages/core/src/tool/write.ts`)

Full-file overwrite with:

- **Location-scoped path resolution** (`LocationMutation.resolve`) —
  relative paths resolve within the active Location; external absolute paths
  require `external_directory` approval. Rejects escape attempts and symlink
  traversal.
- **Permission gating** (`assertPermission`) — user must approve the edit
  action before write executes. External paths get a separate
  `externalDirectoryPermission` approval first.
- **BOM-preserving write** (`files.writeTextPreservingBom`) — reads existing
  file BOM bytes, preserves them on write, emits at most one BOM.
- **Keyed mutex** — `KeyedMutex` per canonical target serializes writes.
- **Revalidation** — immediately before filesystem write, re-verifies the plan
  (identity, canonical path, resource) to detect mid-flight races.
- **Structured success** — returns `{ operation, target, resource, existed }`.

### V2 `edit` (`packages/core/src/tool/edit.ts`)

Partial file edit via exact string replacement with:

- All of the above path resolution, permission gating, and locking.
- **Exact oldString/newString/replaceAll** parameters.
- **Line-ending normalization** — detects the target file's existing line ending
  style (`\n` vs `\r\n`), normalizes both `oldString` and `newString` to match
  before matching.
- **BOM detection and preservation** — splits BOM from content before matching,
  re-joins on write.
- **Occurrence counting** — rejects if `oldString` not found; rejects if
  multiple matches found and `replaceAll !== true`.
- **Stale-content guard** (`writeIfUnchanged`) — under lock, re-reads the file
  and only writes if the bytes are unchanged since initial read. Returns
  `StaleContentError` on mismatch.
- **Diff preview in model output** — returns a unified-diff snippet showing
  removed and added lines.
- **Structured success** — includes a `replacements` count.

### V2 `apply_patch` (`packages/core/src/tool/apply-patch.ts`)

Multi-operation patch (add, update, delete) with upfront plan resolution and
approval, then uninterruptible sequential application.

---

## Target: OpenCode V1 Tools

OpenCode V1 lives in `packages/opencode/src/tool/`. These have richer
integrations that V2 has deferred.

### V1 `write` (`packages/opencode/src/tool/write.ts`)

Full-file overwrite with all V2 features plus:

- **Diff preview before permission approval** — generates a unified diff via
  `createTwoFilesPatch` and shows it in the permission prompt.
- **Auto-format after write** — runs `format.file(filepath)` after writing;
  re-syncs BOM if format changes it.
- **LSP diagnostics after write** — calls `lsp.touchFile(filepath)`, collects
  diagnostics, and reports them in the output (up to 5 files).
- **Event publishing** — publishes `FileSystem.Event.Edited` and
  `Watcher.Event.Updated`.

### V1 `edit` (`packages/opencode/src/tool/edit.ts`)

Partial edit with all V2 features plus:

- **Fuzzy correction strategies** (8 replacers tried in order):
  1. `SimpleReplacer` — exact match.
  2. `LineTrimmedReplacer` — trimmed line matching.
  3. `BlockAnchorReplacer` — first/last line anchor matching with Levenshtein
     similarity threshold.
  4. `WhitespaceNormalizedReplacer` — normalized whitespace.
  5. `IndentationFlexibleReplacer` — ignores indentation.
  6. `EscapeNormalizedReplacer` — unescaped string matching.
  7. `TrimmedBoundaryReplacer` — trimmed boundary matching.
  8. `ContextAwareReplacer` — first/last line context anchors with 50%
     middle-line similarity.
  9. `MultiOccurrenceReplacer` — finds all exact occurrences.
- **Levenshtein distance** for similarity scoring.
- **Disproportionate match detection** — rejects matches that are too large
  relative to oldString.
- **Per-file semaphore** (not `KeyedMutex`) for serialization.
- **Snapshot / undo metadata** — captures `filediff` metadata for undo support.
- **Diff preview in permission** and **diff preview in model output**.
- **Auto-format**, **LSP diagnostics**, **event publishing** (same as V1 write).

---

## Parity Gap Analysis

The gap can be grouped into layers of increasing sophistication.

### Layer 1: Add the `edit` Tool (Highest Value, Lowest Effort)

Probe has no way to do partial file edits. The model must read, edit in its
context window, and write back the entire file. This is wasteful and error-prone.

**What to build:**

A new tool (name: `edit` or `edit_file`) at `packages/runtime/src/tool/edit.ts`
with parameters:

| Param        | Type    | Required | Description                           |
|--------------|---------|----------|---------------------------------------|
| `path`       | string  | yes      | Relative file path under workspace    |
| `oldString`  | string  | yes      | Exact text to replace                 |
| `newString`  | string  | yes      | Replacement text (must differ)        |
| `replaceAll` | boolean | no       | Replace all exact occurrences (false) |

Implementation:

1. Reuse `resolveWorkspacePath` from the existing `write_file` handler.
2. Reject if `oldString` is empty (use `write_file` to create files).
3. Reject if `oldString === newString`.
4. Read the file from disk.
5. Count occurrences of `oldString`; reject if 0 matches; reject if >1 and
   `replaceAll !== true`.
6. Perform the replacement.
7. Write the result back (full file write, but only the modified content).

That alone closes the biggest gap. Total new code: ~80-120 lines.

### Layer 1b: Add `apply_patch` Tool (Medium Value)

A multi-operation patch tool that accepts a complete patch description with add,
update, and delete operations. Useful when the model wants to make several
changes to a file in one tool call. Can be built after the basic `edit` tool.

### Layer 2: Permission / Approval Gating

Probe writes immediately with no user-in-the-loop. OpenCode requires explicit
user approval for every mutation.

**What to build:**

1. Define a permission/approval service. The tool handler yields a request that
   the CLI presents to the user (`Allow/Deny/Always allow`) before continuing.
2. For each write/edit, compute and display a unified diff so the user can see
   what changed.
3. Support a "save" / "always allow" pattern (OpenCode's `save: ["*"]`).

**Interface sketch:**

```typescript
interface ApprovalContext {
  ask(request: {
    action: "edit";
    file: string;
    diff: string;
  }): Effect.Effect<"allow" | "deny" | "always", never>
}
```

### Layer 3: BOM Handling

UTF-8 BOM is rare but real. If probe writes a file that previously had a BOM
without preserving it, some tools and compilers break.

**What to build:**

1. Before writing, read the first 3 bytes of the existing file.
2. If they match `0xEF 0xBB 0xBF`, the file has a BOM.
3. When writing new content, strip any user-provided BOM, then prepend the BOM
   if the original file had one (or if the user explicitly provided one).
4. Do the same for the `edit` tool: split BOM before matching oldString,
   rejoin after replacement.

### Layer 4: Line-Ending Normalization

If the model sends `oldString` with `\n` but the file has `\r\n`, the match
fails. OpenCode normalizes both sides to `\n`, matches, then converts the
result back to the file's existing line ending before writing.

**What to build in the `edit` tool:**

1. Detect the file's line ending style: `file.includes("\r\n") ? "\r\n" : "\n"`.
2. Normalize `oldString` and `newString` to `\n` (strip `\r`).
3. Normalize the file content to `\n`.
4. Match and replace.
5. Convert the result back to the detected line ending.

### Layer 5: Stale-Content Guard

Between the time the model reads a file and calls `edit`, another actor might
have changed it. Without a guard, probe silently overwrites the intermediate
change.

**What to build in the `edit` tool:**

1. Before reading, record the file content's byte hash or the full byte array.
2. Before writing, under a per-file lock, re-read the file and compare bytes.
3. If they differ, fail with a clear error: "File changed since read. Read it
   again before editing."

### Layer 6: Per-File Locking / Mutex

If the model issues two concurrent `write_file` or `edit` calls to the same
file, they race.

**What to build:**

Wrap the critical section (read → replace → write) in a per-canonical-target
mutex. OpenCode uses `KeyedMutex` (async mutex keyed by string). Probe already
uses Effect; `Effect.lock` or a simple `Map<string, Semaphore>` works.

### Layer 7: Revalidation After Plan Resolution

OpenCode V2 resolves a "plan" (target identity, canonical path, resource) early,
then revalidates it immediately before the filesystem write. This catches
mid-flight races where the file was moved, deleted, or replaced with a symlink.

**What to build:**

1. Resolve the plan early (for permission approval).
2. After approval but before write, re-read the filesystem identity at the same
   path and verify it matches the plan (same dev/ino or canonical realpath).
3. Fail with a clear error if it changed.

Probe's simpler `resolveWorkspacePath` already resolves to an absolute path.
Revalidation means re-doing the resolution + stat check just before writing.

### Layer 8: LSP Diagnostics After Write

After writing or editing a file, OpenCode V1 touches the file with LSP
(`lsp.touchFile(filepath, "document")`), collects diagnostics, and reports
them in the tool output. This gives the model immediate feedback about errors
it introduced.

**What to build:**

1. After a successful write/edit, send a notification to the language server
   about the changed file.
2. Collect diagnostics for that file (and up to N other files that got new
   diagnostics).
3. Include the diagnostics in the tool output so the model sees them.

This depends on Probe having an LSP service. If one does not exist yet, this is
a larger prerequisite.

### Layer 9: Auto-Format After Write

OpenCode V1 runs the project's formatter on the written file (e.g., Prettier,
dprint) and re-syncs the BOM if the formatter changed it.

**What to build:**

1. After writing, detect the applicable formatter for the file type.
2. Run it.
3. Re-check BOM after formatting (formatters may strip it).

Depends on Probe having a format service.

### Layer 10: Event Publishing For Watchers

OpenCode publishes filesystem events so that file watchers, the TUI file tree,
and other UI elements know about changes.

**What to build:**

Publish a `FileSystem.Event.Edited` and `Watcher.Event.Updated` event after
each mutation. Depends on Probe having an event system.

### Layer 11: Snapshot / Undo Metadata

OpenCode V1 captures `filediff` metadata in its snapshot system after each
edit, enabling undo.

**What to build:**

1. Before writing, capture a copy of the old file content and a diff.
2. Store it in a snapshot/undo service keyed by file path.
3. The UI can then offer an undo command that reverts the last edit.

### Layer 12: Fuzzy Correction Strategies

When the model's `oldString` does not match the file exactly (wrong indentation,
extra whitespace, different quoting), OpenCode V1 tries 8 increasingly relaxed
replacer strategies before giving up. This reduces model errors.

**What to build:**

A replacer pipeline:

```typescript
type Replacer = {
  name: string;
  try: (content: string, oldString: string, newString: string) =>
    { replaced: string; count: number } | undefined;
};
```

Try replacers in order:

1. Exact match (simple string replace).
2. Line-trimmed: trim each line before matching.
3. Block-anchor: require first and last lines to match with high Levenshtein
   similarity; inner lines can vary more.
4. Whitespace-normalized: collapse all whitespace runs to single spaces.
5. Indentation-flexible: ignore leading whitespace differences.
6. Escape-normalized: unescape both strings before matching.
7. Trimmed-boundary: trim whitespace from oldString boundaries.
8. Context-aware: require anchor lines at top and bottom with 50% similarity
   on middle lines.
9. Multi-occurrence: if `replaceAll` is false but a unique replacement can be
   inferred from context.

Each replacer must return a similarity score so the system can detect
disproportionate matches (e.g., matching 500 chars when oldString was 10).

### Layer 13: Structured Error Types

OpenCode uses `Schema.TaggedErrorClass` for typed errors like
`StaleContentError`, `TargetExistsError`, `LocationMutation.RevalidationError`,
and `FSUtil.Error`. These are caught by the tool framework and projected into
the `ToolFailure` response with clear messages.

Probe has `ProbeLlmToolFailure` (one class with a `message` string).
Adding typed error variants lets the tool handler give more specific error
messages and lets callers distinguish error types.

### Layer 14: Structured Success Types

OpenCode V2 returns a typed success object (`{ operation, target, resource, existed }`)
rather than a natural-language string. The `toModelOutput` function projects this
into model-readable text. Probe returns `{ path, content: "written to ..." }`.

Moving to structured success types makes the tool output parseable by other
tools and improves the model's understanding of what happened.

### Layer 15: `apply_patch` Multi-Operation Tool

OpenCode V2 has an `apply_patch` tool that accepts a complete patch with add,
update, and delete operations targeting multiple files. It resolves all plans
up front, approves all mutations once, then applies them sequentially without
interruption.

This is a separate tool, not a replacement for `write`/`edit`. Building it
would let the model express multi-file changes in one turn.

---

## Priority Order

| Priority | Layer | Effort | Value | Notes |
|----------|-------|--------|-------|-------|
| P0       | 1  — `edit` tool          | small  | high  | Closes the biggest functional gap |
| P0       | 1b — `apply_patch` tool   | medium | med   | Useful for multi-file changes |
| P1       | 2  — permission/approval  | medium | high  | Safety; no silent file mutations |
| P1       | 3  — BOM handling         | small  | low   | Edge case but breaks tools when wrong |
| P1       | 4  — line-ending norm     | small  | med   | Prevents false "oldString not found" |
| P1       | 5  — stale-content guard  | small  | med   | Prevents silent overwrites |
| P1       | 6  — per-file locking     | small  | med   | Prevents races |
| P2       | 7  — revalidation         | small  | med   | Catches mid-flight path changes |
| P2       | 8  — LSP diagnostics      | large  | high  | Great for model feedback but depends on LSP |
| P2       | 9  — auto-format          | medium | med   | Depends on format service |
| P3       | 10 — event publishing     | medium | low   | Depends on event system |
| P3       | 11 — snapshot/undo        | medium | low   | Nice to have |
| P3       | 12 — fuzzy correction     | large  | med   | Tolerates model errors |
| P3       | 13 — structured errors   | small  | low   | Incremental polish |
| P3       | 14 — structured success  | small  | low   | Incremental polish |

Total effort to reach full V2 parity (P0+P1): about 2-3 focused sessions.
Total effort to reach full V1 parity (P0-P3): about 1-2 weeks depending on
LSP/format service readiness.

---

## File-Level Checklist

The following checklist enumerates every specific change needed. Check off items
as they are completed.

### New Tool: `edit` (`packages/runtime/src/tool/edit.ts`)

- [ ] Define tool parameters: `path`, `oldString`, `newString`, `replaceAll?`
- [ ] Register in the tool list alongside `write_file`
- [ ] Resolve path via existing `resolveWorkspacePath`
- [ ] Reject empty `oldString` (use `write_file` instead)
- [ ] Reject `oldString === newString`
- [ ] Reject if `oldString` not found in file
- [ ] Reject if multiple matches found and `replaceAll !== true`
- [ ] Perform exact text replacement
- [ ] Write result back to disk
- [ ] Return structured result with replacement count

### New Tool: `apply_patch` (`packages/runtime/src/tool/apply-patch.ts`)

- [ ] Define patch format (add/update/delete operations)
- [ ] Resolve all target plans up front
- [ ] Single permission approval for all operations
- [ ] Sequential uninterruptible application
- [ ] Report partial application on failure

### Existing `write_file` Upgrades

- [ ] Add BOM preservation (read existing BOM, preserve on write)
- [ ] Add line-ending normalization (optional; `edit` needs it more)
- [ ] Add per-file locking
- [ ] Add revalidation before write

### All Mutation Tools (shared infra)

- [ ] Build permission/approval service with diff preview
- [ ] Build `KeyedMutex` or semaphore per canonical target
- [ ] Build stale-content guard (`writeIfUnchanged` pattern)
- [ ] Add structured error types (`StaleContentError`, etc.)
- [ ] Add structured success types
- [ ] Build revalidation step (re-check path identity pre-write)
- [ ] Build replacer pipeline with fuzzy correction strategies (P3)

### Integration (depends on other services)

- [ ] LSP service: `touchFile` + collect diagnostics (P2, deps on LSP)
- [ ] Format service: run formatter after write (P2, deps on format)
- [ ] Event system: publish file-change events (P3, deps on events)
- [ ] Snapshot/undo: capture pre-write state (P3)

---

## Appendix: Key Files in Each Repo

### OpenCode V2 Core (`packages/core`)

| File | Purpose |
|------|---------|
| `src/tool/write.ts` | V2 write tool definition + handler |
| `src/tool/edit.ts` | V2 edit tool definition + handler |
| `src/tool/apply-patch.ts` | V2 multi-op patch tool |
| `src/file-mutation.ts` | File mutation service: create/write/writeTextPreservingBom/writeIfUnchanged/remove with locking + revalidation |
| `src/location-mutation.ts` | Plan resolution: path → canonical target + resource + revalidation |
| `src/fs-util.ts` | Filesystem utility service (readFile, writeFile, writeWithDirs, etc.) |
| `src/effect/keyed-mutex.ts` | Async mutex keyed by string |

### OpenCode V1 Opencode (`packages/opencode`)

| File | Purpose |
|------|---------|
| `src/tool/write.ts` | V1 write tool with LSP, format, events, diff preview in permission |
| `src/tool/edit.ts` | V1 edit tool with 8 fuzzy replacers, Levenshtein, snapshots, semaphore locks |
| `src/util/bom.ts` | BOM split/join/read-file/sync-file utilities |
| `src/snapshot/index.ts` | Snapshot/undo metadata storage |

### Probe (`packages/runtime`)

| File | Purpose |
|------|---------|
| `src/cli.ts` | Current `write_file` tool definition (line 1073) + handler (line 1197) + `resolveWorkspacePath` (line 1372) |
| `src/llm/tool.ts` | `ProbeLlmTool` type + `defineProbeLlmTool` helper |
