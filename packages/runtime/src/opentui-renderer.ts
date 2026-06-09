import {
  createCliRenderer,
  MarkdownRenderable,
  CodeRenderable,
  DiffRenderable,
  BoxRenderable,
  TextRenderable,
  ScrollBoxRenderable,
  LineNumberRenderable,
  SyntaxStyle,
  parseColor,
  type CliRenderer,
} from "@opentui/core";

export interface ProbeRenderer {
  readonly renderer: CliRenderer;
  readonly syntaxStyle: SyntaxStyle;
  readonly session: ScrollBoxRenderable;
  readonly text: MarkdownRenderable;
}

export { parseColor, TextRenderable, ScrollBoxRenderable, BoxRenderable, DiffRenderable, LineNumberRenderable } from "@opentui/core";

export function createDefaultSyntaxStyle(): SyntaxStyle {
  return SyntaxStyle.fromStyles({
    keyword: { fg: parseColor("#FF7B72"), bold: true },
    string: { fg: parseColor("#A5D6FF") },
    comment: { fg: parseColor("#8B949E"), italic: true },
    number: { fg: parseColor("#79C0FF") },
    function: { fg: parseColor("#D2A8FF") },
    type: { fg: parseColor("#FFA657") },
    operator: { fg: parseColor("#FF7B72") },
    variable: { fg: parseColor("#E6EDF3") },
    property: { fg: parseColor("#79C0FF") },
    bracket: { fg: parseColor("#F0F6FC") },
    delimiter: { fg: parseColor("#C9D1D9") },
    "markup.heading": { fg: parseColor("#00D7FF"), bold: true },
    "markup.bold": { fg: parseColor("#F0F6FC"), bold: true },
    "markup.italic": { fg: parseColor("#F0F6FC"), italic: true },
    "markup.list": { fg: parseColor("#FF7B72") },
    "markup.quote": { fg: parseColor("#8B949E"), italic: true },
    "markup.raw": { fg: parseColor("#A5D6FF"), bg: parseColor("#161B22") },
    "markup.link": { fg: parseColor("#58A6FF"), underline: true },
    "markup.link.url": { fg: parseColor("#58A6FF"), underline: true },
    conceal: { fg: parseColor("#6E7681") },
    default: { fg: parseColor("#E6EDF3") },
  });
}

export function createProbeRenderer(): Promise<CliRenderer> {
  return createCliRenderer({
    exitOnCtrlC: true,
    targetFps: 30,
    screenMode: "main-screen",
  });
}

export function createAssistantText(renderer: CliRenderer): MarkdownRenderable {
  return new MarkdownRenderable(renderer, {
    content: "",
    syntaxStyle: createDefaultSyntaxStyle(),
    conceal: true,
    internalBlockMode: "top-level",
    streaming: true,
    width: "100%",
  });
}

export function createToolOutput(renderer: CliRenderer, content: string, filetype?: string): CodeRenderable {
  return new CodeRenderable(renderer, {
    content,
    filetype: filetype ?? "plaintext",
    syntaxStyle: createDefaultSyntaxStyle(),
    width: "100%",
  });
}
