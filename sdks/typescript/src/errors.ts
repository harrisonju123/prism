export class PrismError extends Error {
  readonly statusCode: number | undefined;
  readonly response: unknown;

  constructor(message: string, statusCode?: number, response?: unknown) {
    super(message);
    this.name = "PrismError";
    this.statusCode = statusCode;
    this.response = response;
  }
}

export class BudgetExceededError extends PrismError {
  constructor(message: string, statusCode?: number, response?: unknown) {
    super(message, statusCode, response);
    this.name = "BudgetExceededError";
  }
}

export class RateLimitError extends PrismError {
  constructor(message: string, statusCode?: number, response?: unknown) {
    super(message, statusCode, response);
    this.name = "RateLimitError";
  }
}

export class CircuitOpenError extends PrismError {
  constructor(message: string, statusCode?: number, response?: unknown) {
    super(message, statusCode, response);
    this.name = "CircuitOpenError";
  }
}

export function raiseForStatus(statusCode: number, body: unknown): void {
  const errBody = body as Record<string, unknown> | null;
  const errObj = errBody?.error as Record<string, unknown> | undefined;
  const msg = (errObj?.message as string) || "unknown error";

  if (statusCode === 429) throw new RateLimitError(msg, statusCode, body);
  if (statusCode === 402) throw new BudgetExceededError(msg, statusCode, body);
  if (statusCode === 503) throw new CircuitOpenError(msg, statusCode, body);
  if (statusCode >= 400) throw new PrismError(msg, statusCode, body);
}
