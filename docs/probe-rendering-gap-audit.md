# Probe Rendering Gap Audit

**Date**: 2026-06-08  
**Reference**: Opencode rendering system at `projects/repos/opencode/`

---

## Current State

Probe's entire terminal output pipeline lives in
`packages/runtime/src/cli.ts` (~1741 lines). Rendering is hand-rolled ANSI
escape sequences and a custom `marked.Renderer` subclass. There is no TUI
framework, no syntax highlighting, no proper diff rendering, no session
transcript view, and no component tree.

The current markdown renderer (`renderMarkdown`, lines 847-872) handles bold,
italic, inline code, links, blockquotes, headings, and lists with crude ANSI
wrapping. Code blocks are rendered in plain gray with no syntax highlighting.
Inline markdown streaming (`formatInlineMarkdown`, lines 874-893) uses regex
replacement rather than a parser.

The diff preview (`createDiffPreview` in `file-mutation.ts`, lines 53-70) is a
manual line-by-line comparison with `+`/`-` prefixes, truncated to 10 lines
each. Permission prompts filter these lines further to only `+`/`-` prefixed
lines, with no color.

---

## Opencode's Approach (Reference)

| Surface | Technology | Key Features |
|---|---|---|
| TUI | `@opentui/core` (native Zig terminal engine) | `CodeRenderable`, `DiffRenderable`, `MarkdownRenderable`, `LineNumberRenderable`; tree-sitter WASM syntax highlighting; split/unified diff views; hunk jumping; line numbers; concealment for streaming |
| Web App | SolidJS + `@pierre/diffs` + Shiki | `marked` + `marked-shiki` for markdown; `DiffChanges` components; interactive review UI with line comments; Ghostty terminal emulator |
| Web Share | Astro/SolidJS + Shiki | `codeToHtml` with GitHub themes; side-by-side diff |

### Syntax Highlighting — Tree-sitter WASM (TUI)

Opencode loads tree-sitter WASM parsers at runtime for 30+ languages
(`packages/opencode/parsers-config.ts`). The TUI `CodeRenderable` consumes
`SimpleHighlight[]` arrays with `fg`, `bg`, `bold`, `italic`, `underline`
attributes. A fallback path uses Shiki's `codeToHtml` for web surfaces.

Language is detected from file extensions via `LANGUAGE_EXTENSIONS` map in
`packages/opencode/src/lsp/language.ts`.

### Diff Rendering — `DiffRenderable` (TUI)

Opencode's TUI diff viewer (`packages/opencode/src/cli/cmd/tui/feature-plugins/system/diff-viewer.tsx`) is a full modal with:

- File tree sidebar with expandable directories
- Split and unified views (auto-adapts to terminal width)
- Hunk jumping (next/previous)
- File marking (reviewed, single-patch toggling)
- Source switching (working tree vs last turn)

Under the hood (`@opentui/core`):

1. **Parsing**: Uses `parsePatch` from the `diff` npm package to produce a
   `StructuredPatch` with hunks.
2. **Unified view**: Iterates hunks/lines. `+` lines get green background and
   ` +` sign; `-` lines get red background and ` -` sign; context lines get
   neutral background and both line numbers incremented. Content is wrapped in
   `CodeRenderable` then `LineNumberRenderable`.
3. **Split view**: Two 50%-width panes (left = removed, right = added), with
   paired `-`/`+` lines aligned via padding. Sync-scrolled.
4. **Colors**: Theme-driven (`diffAddedBg`, `diffRemovedBg`, etc.) with
   dimmed colors for reviewed lines.

### Markdown Streaming

The web app (`packages/ui/src/components/markdown-stream.ts`) splits markdown
into blocks, detects unclosed fenced code blocks during streaming, heals
incomplete links, and returns blocks with `"full"` or `"live"` mode. The
`Markdown` component renders each block through `marked` and reconciles the
DOM incrementally with `morphdom` (preserving copy button state).

---

## Gap Analysis

### Gap 1: No Syntax Highlighting

**Severity**: High  
**Current**: All code blocks are plain gray. The `_language` parameter to
`marked.Renderer.codes()` is ignored.  
**Impact**: Code in tool results, assistant responses, and permission diffs is
unreadable. The most important signal for a coding agent — the code itself —
has zero visual structure.  
**Opencode**: Tree-sitter WASM with 30+ language parsers, language detection
from file extensions, `SimpleHighlight[]` arrays with fg/bg/bold/italic.  
**Path**: Wire a syntax highlighter into `cli.ts`:

1. Pick a lightweight highlighter: `shiki` (used by opencode) or a simpler
   tokenizer like `highlight.js` or `Prism` for terminal output.
2. Replace `renderMarkdown`'s `codes()` method to call the highlighter and
   emit ANSI-colored spans per token.
3. For inline streaming (`formatInlineMarkdown`), do the same in the
   regex-based code-fence detection (though this currently strips code blocks
   entirely).
4. Consider adding a `--language` / `filetype` hint for raw code display.

---

### Gap 2: No Proper Diff Rendering

**Severity**: High  
**Current**: `createDiffPreview()` compares lines positionally (no LCS/Myers
diff algorithm), shows at most 10 old + 10 new lines, truncates at 200 chars,
and renders with plain `+`/`-` prefixes (no color). Permission prompts filter
to only `+`/`-` lines, losing context.  
**Impact**: Users cannot see what changed in a file edit. The diff shown for
permission approval is essentially useless for anything but the simplest
single-line changes.  
**Opencode**: Full unified/split view with:

- `parsePatch` from the `diff` npm package (Myers diff algorithm)
- Hunk-aware rendering with context lines
- Colored backgrounds (green for added, red for removed)
- Gutter signs (` +` / ` -`) in matching colors
- Line numbers for old and new
- Hunk headers (e.g., `@@ -10,7 +10,8 @@`)

**Path**:

1. Use the `diff` npm package (already a dependency of `marked`) to generate
   a proper `StructuredPatch` from old/new text.
2. Build a `renderDiff()` function in `cli.ts` that:
   - Parses the patch into hunks
   - For each hunk, prints the `@@` header in muted yellow
   - For each line, prints `+` in green (with green background ANSI), `-` in
     red (with red background ANSI), ` ` context in default
   - Shows line numbers (old/new) in gray
3. Replace `createDiffPreview()` to use this renderer.
4. Update `makeCliPermissionHandler()` to pass through the rendered diff
   (with color) instead of filtering to only `+`/`-` lines.

---

### Gap 3: No TUI Framework

**Severity**: Medium  
**Current**: Flat `process.stdout.write()` with hand-rolled ANSI codes. No
layout system, no scrolling, no paging, no viewport management.  
**Impact**: Long outputs are unreadable (scroll off screen). There is no way
to interact with output (scroll back, search, expand/collapse). Session
history is lost between turns.  
**Opencode**: `@opentui/core` — a native Zig terminal engine with JSX-like
renderables, viewport management, streaming support, and theme-driven styling.  
**Path**: This is a larger architectural change. Short-term wins:

1. Add `--pager` / `--no-pager` with `$PAGER` env support, piping long output
   through `less -R`.
2. Add a session log file (already partially present in the non-CLI backend
   paths) and a `probe session show <id>` command that replays the transcript.
3. For the full path, consider wrapping the output in a terminal paging
   library like `blessed` (Node.js) or adopting `@opentui/core` if/when probe
   aligns with opencode's rendering stack.

---

### Gap 4: No Session Transcript Display

**Severity**: Medium  
**Current**: The interactive chat loop accumulates messages in a flat array
and re-sends them on each turn. There is no command to view a previous
session, no scroll-back beyond terminal history, no rendered transcript.  
**Impact**: Users cannot review what happened in a previous turn or session.
Debugging is limited to what fits on screen.  
**Opencode**: The TUI session view renders each message part distinctly:
`TextPart` via `<markdown>`, `ReasoningPart` via `<code filetype="markdown">`,
`Write` tool output via `<code>` with line numbers.  
**Path**:

1. Persist session transcripts to a file (JSONL or Markdown).
2. Add `probe session log [--id <id>]` that reads and renders the transcript
   through `renderMarkdown()`.
3. For the rich TUI path, consider a `probe session replay` command.

---

### Gap 5: Tool Output Is Invisible

**Severity**: Medium  
**Current**: `formatToolResultValue()` returns one-line summaries like
`"{path} ({content.length} chars)"`. The actual content read from files or
returned by tools is sent to the LLM but hidden from the user.  
**Impact**: Users cannot see what the agent read, what search results it got,
or what command output it received. This makes debugging agent behavior
nearly impossible.  
**Opencode**: Tool outputs are rendered with `<code>` and line numbers, the
`<diff>` viewer shows file changes with full context.  
**Path**:

1. Add a configurable `--show-tool-output` / `verbose` mode.
2. For `read_file` results, render the full content through the syntax
   highlighter (with detected language from file extension).
3. For `search_code` results, show matches with context lines and line
   numbers (like ripgrep's `--context`).

---

### Gap 6: No ANSI Passthrough / Terminal Emulation

**Severity**: Low  
**Current**: ANSI escape sequences are unconditionally stripped from shell
tool output before display.  
**Impact**: Colored output from tools (e.g., `npm test` with pass/fail colors,
`git diff` with its own coloring) is lost.  
**Opencode**: ANSI is stripped in the TUI (`stripAnsi`), but the web app uses
the Ghostty terminal emulator for full terminal rendering.  
**Path**: Add a `--passthrough-ansi` flag or auto-detect when the output is
likely colored. For now, the stripping behavior is acceptable for most cases.

---

### Gap 7: Streaming Markdown Uses Regex, Not a Parser

**Severity**: Low  
**Current**: `formatInlineMarkdown()` uses simple regex replacements for bold,
inline code, links, etc. It does not handle nested formatting, does not parse
code blocks, and does not handle incomplete streaming content correctly.  
**Impact**: Streaming assistant text can show raw markdown syntax (especially
code fences that span multiple chunks).  
**Opencode**: A dedicated streaming markdown parser (`markdown-stream.ts`)
splits input into blocks, detects open fences, heals incomplete links, and
renders each block independently.  
**Path**: Use `marked.lexer()` incrementally on streaming chunks, detecting
incomplete tokens and healing them before passing to the renderer.

---

## Implementation Priority

| Gap | Severity | Effort | Priority |
|---|---|---|---|
| 1. Syntax highlighting | High | Medium | **P0** |
| 2. Proper diff rendering | High | Medium | **P0** |
| 5. Visible tool output | Medium | Low | **P1** |
| 4. Session transcript display | Medium | Medium | **P1** |
| 3. TUI framework | Medium | High | **P2** |
| 7. Streaming markdown parser | Low | Low | **P2** |
| 6. ANSI passthrough | Low | Low | **P3** |

---

## Recommended First Steps

1. **Syntax highlighting** — Add `shiki` (or `highlight.js` for lighter weight)
   to the render pipeline. Modify `renderMarkdown`'s `codes()` to call the
   highlighter with the language hint and emit ANSI-colored spans. This is the
   single highest-impact change.
2. **Diff rendering** — Use the `diff` npm package (already a transitive
   dependency via `marked`) to compute proper hunks. Write a `renderDiff()`
   function with colored backgrounds and `+`/`-` signs. Replace
   `createDiffPreview()` and update the permission prompt.
3. **Tool output** — Add a `verbose` flag. In verbose mode, render tool
   results (especially `read_file` and `search_code`) in full through the
   syntax highlighter with line numbers.
