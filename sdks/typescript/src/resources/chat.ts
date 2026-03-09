import type { PrismClient } from "../client.js";
import { raiseForStatus } from "../errors.js";

export interface Message {
  role: "system" | "user" | "assistant" | "tool";
  content: string | null;
  name?: string;
  tool_calls?: ToolCall[];
  tool_call_id?: string;
}

export interface ToolCall {
  id: string;
  type: "function";
  function: {
    name: string;
    arguments: string;
  };
}

export interface ChatCompletionRequest {
  model: string;
  messages: Message[];
  stream?: boolean;
  temperature?: number;
  max_tokens?: number;
  tools?: Tool[];
  tool_choice?: unknown;
  [key: string]: unknown;
}

export interface Tool {
  type: "function";
  function: {
    name: string;
    description?: string;
    parameters?: unknown;
  };
}

export interface ChatCompletionChoice {
  index: number;
  message: Message;
  finish_reason: string | null;
}

export interface ChatCompletion {
  id: string;
  object: "chat.completion";
  created: number;
  model: string;
  choices: ChatCompletionChoice[];
  usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

export interface ChatCompletionChunkDelta {
  role?: string;
  content?: string | null;
  tool_calls?: Partial<ToolCall>[];
}

export interface ChatCompletionChunkChoice {
  index: number;
  delta: ChatCompletionChunkDelta;
  finish_reason: string | null;
}

export interface ChatCompletionChunk {
  id: string;
  object: "chat.completion.chunk";
  created: number;
  model: string;
  choices: ChatCompletionChunkChoice[];
}

export type CreateParams = Omit<ChatCompletionRequest, "stream">;

const MAX_RETRIES = 3;
const RETRY_DELAY_MS = 1000;

async function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export class ChatCompletions {
  constructor(private client: PrismClient) {}

  async create(params: CreateParams & { stream?: false }): Promise<ChatCompletion>;
  async create(params: CreateParams & { stream: true }): Promise<AsyncIterable<ChatCompletionChunk>>;
  async create(params: CreateParams & { stream?: boolean }): Promise<ChatCompletion | AsyncIterable<ChatCompletionChunk>>;
  async create(params: CreateParams & { stream?: boolean }): Promise<ChatCompletion | AsyncIterable<ChatCompletionChunk>> {
    if (params.stream) {
      return this._createStream(params);
    }
    return this._createSync(params);
  }

  private async _createSync(params: CreateParams): Promise<ChatCompletion> {
    for (let attempt = 0; attempt < MAX_RETRIES; attempt++) {
      const response = await this.client.fetch("/v1/chat/completions", {
        method: "POST",
        body: JSON.stringify({ ...params, stream: false }),
      });

      if (response.status === 429 && attempt < MAX_RETRIES - 1) {
        await sleep(RETRY_DELAY_MS * Math.pow(2, attempt));
        continue;
      }

      const body = await response.json().catch(() => ({}));
      raiseForStatus(response.status, body);
      return body as ChatCompletion;
    }
    throw new Error("unreachable");
  }

  private async _createStream(params: CreateParams): Promise<AsyncIterable<ChatCompletionChunk>> {
    const response = await this.client.fetch("/v1/chat/completions", {
      method: "POST",
      body: JSON.stringify({ ...params, stream: true }),
    });

    raiseForStatus(response.status, {});

    const body = response.body;
    if (!body) throw new Error("no response body");

    return (async function* () {
      const reader = body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";

      try {
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;

          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split("\n");
          buffer = lines.pop() ?? "";

          for (const line of lines) {
            const trimmed = line.trim();
            if (!trimmed.startsWith("data: ")) continue;
            const data = trimmed.slice(6);
            if (data === "[DONE]") return;
            try {
              yield JSON.parse(data) as ChatCompletionChunk;
            } catch {
              // skip malformed SSE
            }
          }
        }
      } finally {
        reader.releaseLock();
      }
    })();
  }
}

export class Chat {
  readonly completions: ChatCompletions;

  constructor(client: PrismClient) {
    this.completions = new ChatCompletions(client);
  }
}
