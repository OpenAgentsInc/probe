import { Buffer } from "node:buffer";
import { type ProbeLlmContentPart, type ProbeLlmMessage, type ProbeLlmRequest, stringifyToolResult } from "../../llm";
import { type ProbeLlmToolDefinition } from "../../llm/tool";
import { convertProbeToolSchemaToGemini } from "./tool-schema";

export interface GeminiTextPart {
  readonly text: string;
  readonly thought?: boolean;
  readonly thoughtSignature?: string;
}

export interface GeminiInlineDataPart {
  readonly inlineData: {
    readonly mimeType: string;
    readonly data: string;
  };
}

export interface GeminiFunctionCallPart {
  readonly functionCall: {
    readonly name: string;
    readonly args: unknown;
  };
  readonly thoughtSignature?: string;
}

export interface GeminiFunctionResponsePart {
  readonly functionResponse: {
    readonly name: string;
    readonly response: unknown;
  };
}

export type GeminiContentPart =
  | GeminiTextPart
  | GeminiInlineDataPart
  | GeminiFunctionCallPart
  | GeminiFunctionResponsePart;

export interface GeminiContent {
  readonly role: "user" | "model";
  readonly parts: ReadonlyArray<GeminiContentPart>;
}

export interface GeminiFunctionDeclaration {
  readonly name: string;
  readonly description: string;
  readonly parameters?: Record<string, unknown>;
}

export interface GeminiBody {
  readonly contents: ReadonlyArray<GeminiContent>;
  readonly systemInstruction?: {
    readonly parts: ReadonlyArray<{ readonly text: string }>;
  };
  readonly tools?: ReadonlyArray<{
    readonly functionDeclarations: ReadonlyArray<GeminiFunctionDeclaration>;
  }>;
  readonly toolConfig?: {
    readonly functionCallingConfig: {
      readonly mode: "AUTO" | "NONE" | "ANY";
      readonly allowedFunctionNames?: ReadonlyArray<string>;
    };
  };
  readonly generationConfig?: {
    readonly maxOutputTokens?: number;
    readonly temperature?: number;
    readonly topP?: number;
    readonly topK?: number;
    readonly stopSequences?: ReadonlyArray<string>;
    readonly thinkingConfig?: {
      readonly thinkingBudget?: number;
      readonly includeThoughts?: boolean;
    };
  };
}

export function geminiEndpointPath(modelId: string): string {
  return `/models/${encodeURIComponent(modelId)}:streamGenerateContent?alt=sse`;
}

export function lowerProbeLlmRequestToGeminiBody(request: ProbeLlmRequest): GeminiBody {
  const contents = lowerMessages(request.messages);
  const systemText = request.system.flatMap(messageTextParts).join("\n\n");
  const toolsEnabled = request.tools.length > 0 && request.toolChoice?.type !== "none";
  const generationConfig = lowerGenerationConfig(request);

  return dropUndefined({
    contents,
    systemInstruction: systemText.length > 0 ? { parts: [{ text: systemText }] } : undefined,
    tools: toolsEnabled ? [{ functionDeclarations: request.tools.map(lowerTool) }] : undefined,
    toolConfig: toolsEnabled && request.toolChoice !== undefined ? lowerToolConfig(request.toolChoice) : undefined,
    generationConfig,
  });
}

function lowerMessages(messages: ReadonlyArray<ProbeLlmMessage>): ReadonlyArray<GeminiContent> {
  const contents: GeminiContent[] = [];

  for (const message of messages) {
    if (message.role === "system") {
      const part = { text: wrapSystemUpdate(messageTextParts(message).join("\n\n")) };
      const previous = contents.at(-1);

      if (previous?.role === "user") {
        contents[contents.length - 1] = { role: "user", parts: [...previous.parts, part] };
      } else {
        contents.push({ role: "user", parts: [part] });
      }
      continue;
    }

    if (message.role === "user") {
      contents.push({ role: "user", parts: message.content.flatMap(lowerUserPart) });
      continue;
    }

    if (message.role === "assistant") {
      contents.push({ role: "model", parts: message.content.flatMap(lowerAssistantPart) });
      continue;
    }

    contents.push({ role: "user", parts: message.content.flatMap(lowerToolResultPart) });
  }

  return contents;
}

function lowerTool(tool: ProbeLlmToolDefinition): GeminiFunctionDeclaration {
  return dropUndefined({
    name: tool.name,
    description: tool.description,
    parameters: convertProbeToolSchemaToGemini(tool.inputSchema),
  });
}

function lowerToolConfig(toolChoice: NonNullable<ProbeLlmRequest["toolChoice"]>): GeminiBody["toolConfig"] {
  if (toolChoice.type === "none") {
    return { functionCallingConfig: { mode: "NONE" } };
  }

  if (toolChoice.type === "required") {
    return { functionCallingConfig: { mode: "ANY" } };
  }

  if (toolChoice.type === "tool") {
    return { functionCallingConfig: { mode: "ANY", allowedFunctionNames: [toolChoice.name] } };
  }

  return { functionCallingConfig: { mode: "AUTO" } };
}

function lowerGenerationConfig(request: ProbeLlmRequest): GeminiBody["generationConfig"] {
  const generationConfig = dropUndefined({
    maxOutputTokens: request.generation?.maxTokens,
    temperature: request.generation?.temperature,
    topP: request.generation?.topP,
    topK: request.generation?.topK,
    stopSequences: request.generation?.stop,
    thinkingConfig: lowerThinkingConfig(request.providerOptions?.gemini?.thinkingConfig),
  });

  return Object.keys(generationConfig).length === 0 ? undefined : generationConfig;
}

function lowerThinkingConfig(input: unknown): NonNullable<GeminiBody["generationConfig"]>["thinkingConfig"] {
  if (!isRecord(input)) {
    return undefined;
  }

  const thinkingConfig = dropUndefined({
    thinkingBudget: typeof input.thinkingBudget === "number" ? input.thinkingBudget : undefined,
    includeThoughts: typeof input.includeThoughts === "boolean" ? input.includeThoughts : undefined,
  });

  return Object.keys(thinkingConfig).length === 0 ? undefined : thinkingConfig;
}

function lowerUserPart(part: ProbeLlmContentPart): ReadonlyArray<GeminiContentPart> {
  if (part.type === "text") {
    return [{ text: part.text }];
  }

  if (part.type === "media") {
    return [
      {
        inlineData: {
          mimeType: part.mediaType,
          data: typeof part.data === "string" ? part.data : Buffer.from(part.data).toString("base64"),
        },
      },
    ];
  }

  return [];
}

function lowerAssistantPart(part: ProbeLlmContentPart): ReadonlyArray<GeminiContentPart> {
  if (part.type === "text") {
    return [{ text: part.text }];
  }

  if (part.type === "reasoning") {
    return [
      dropUndefined({
        text: part.text,
        thought: true,
        thoughtSignature: readThoughtSignature(part.providerMetadata),
      }),
    ];
  }

  if (part.type === "tool-call") {
    return [
      dropUndefined({
        functionCall: {
          name: part.name,
          args: part.input,
        },
        thoughtSignature: readThoughtSignature(part.providerMetadata),
      }),
    ];
  }

  return [];
}

function lowerToolResultPart(part: ProbeLlmContentPart): ReadonlyArray<GeminiContentPart> {
  if (part.type !== "tool-result") {
    return [];
  }

  return [
    {
      functionResponse: {
        name: part.name,
        response: {
          name: part.name,
          content: stringifyToolResult(part.result.value),
        },
      },
    },
  ];
}

function messageTextParts(message: ProbeLlmMessage): ReadonlyArray<string> {
  return message.content.flatMap((part) => (part.type === "text" ? [part.text] : []));
}

function wrapSystemUpdate(text: string): string {
  return `<system-update>\n${text}\n</system-update>`;
}

function readThoughtSignature(metadata: Readonly<Record<string, unknown>> | undefined): string | undefined {
  const google = metadata?.google;

  return isRecord(google) && typeof google.thoughtSignature === "string" ? google.thoughtSignature : undefined;
}

function dropUndefined<T extends Readonly<Record<string, unknown>>>(input: T): T {
  return Object.fromEntries(Object.entries(input).filter((entry) => entry[1] !== undefined)) as T;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
