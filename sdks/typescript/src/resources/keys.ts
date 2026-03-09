import type { PrismClient } from "../client.js";
import { raiseForStatus } from "../errors.js";

export interface CreateKeyRequest {
  name: string;
  rpm_limit?: number;
  tpm_limit?: number;
  daily_budget_usd?: number;
  monthly_budget_usd?: number;
  allowed_models?: string[];
  rotation_interval_days?: number;
  [key: string]: unknown;
}

export interface VirtualKey {
  id: string;
  name: string;
  key_prefix: string;
  is_active: boolean;
  rpm_limit?: number;
  tpm_limit?: number;
  daily_budget_usd?: number;
  monthly_budget_usd?: number;
  allowed_models: string[];
  created_at: string;
  expires_at?: string;
}

export interface CreateKeyResponse extends VirtualKey {
  key: string; // plaintext, returned once
}

export class Keys {
  constructor(private client: PrismClient) {}

  async create(params: CreateKeyRequest): Promise<CreateKeyResponse> {
    const response = await this.client.fetch("/api/v1/keys", {
      method: "POST",
      body: JSON.stringify(params),
    });
    const body = await response.json().catch(() => ({}));
    raiseForStatus(response.status, body);
    return body as CreateKeyResponse;
  }

  async list(): Promise<VirtualKey[]> {
    const response = await this.client.fetch("/api/v1/keys");
    const body = await response.json().catch(() => []);
    raiseForStatus(response.status, Array.isArray(body) ? {} : body);
    return Array.isArray(body) ? body : (body as { keys: VirtualKey[] }).keys ?? [];
  }

  async revoke(keyId: string): Promise<void> {
    const response = await this.client.fetch(`/api/v1/keys/${keyId}`, {
      method: "DELETE",
    });
    raiseForStatus(response.status, {});
  }

  async rotate(keyId: string): Promise<CreateKeyResponse> {
    const response = await this.client.fetch(`/api/v1/keys/${keyId}/rotate`, {
      method: "POST",
    });
    const body = await response.json().catch(() => ({}));
    raiseForStatus(response.status, body);
    return body as CreateKeyResponse;
  }

  async usage(keyId: string): Promise<unknown> {
    const response = await this.client.fetch(`/api/v1/keys/${keyId}/usage`);
    const body = await response.json().catch(() => ({}));
    raiseForStatus(response.status, body);
    return body;
  }
}
