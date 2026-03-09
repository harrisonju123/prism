import { Chat } from "./resources/chat.js";
import { Keys } from "./resources/keys.js";

export interface PrismClientOptions {
  baseUrl?: string;
  apiKey?: string;
  timeout?: number;
}

export class PrismClient {
  readonly baseUrl: string;
  readonly apiKey: string;
  readonly chat: Chat;
  readonly keys: Keys;

  constructor(options: PrismClientOptions = {}) {
    this.baseUrl = (
      options.baseUrl ??
      (typeof process !== "undefined"
        ? process.env["PRISM_URL"]
        : undefined) ??
      "http://localhost:9100"
    ).replace(/\/$/, "");

    this.apiKey =
      options.apiKey ??
      (typeof process !== "undefined"
        ? process.env["PRISM_API_KEY"]
        : undefined) ??
      "";

    this.chat = new Chat(this);
    this.keys = new Keys(this);
  }

  async fetch(path: string, init: RequestInit = {}): Promise<Response> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      "User-Agent": "prism-gateway-typescript/0.1",
      ...(init.headers as Record<string, string> | undefined),
    };
    if (this.apiKey) {
      headers["Authorization"] = `Bearer ${this.apiKey}`;
    }
    return globalThis.fetch(url, { ...init, headers });
  }

  async health(): Promise<unknown> {
    const response = await this.fetch("/health");
    return response.json();
  }

  async models(): Promise<unknown> {
    const response = await this.fetch("/v1/models");
    return response.json();
  }
}
