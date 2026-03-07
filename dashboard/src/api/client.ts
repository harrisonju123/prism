import type {
  StatsSummary,
  TimeseriesResponse,
  TopTrace,
  WasteScore,
  EventsResponse,
  LLMEvent,
  AlertRule,
  AlertRuleCreate,
  AlertHistoryResponse,
  Budget,
  BudgetCreate,
  FitnessMatrixResponse,
  BenchmarkResultsResponse,
  BenchmarkConfig,
  DriftResponse,
  PolicyResponse,
  DecisionsResponse,
  DryRunReport,
  MCPServersResponse,
  MCPGraphResponse,
  MCPWasteResponse,
} from "./types";

const API_BASE = "/api/v1";

async function throwIfNotOk(res: Response): Promise<void> {
  if (res.ok) return;
  const body = await res.text().catch(() => "");
  throw new Error(`API error: ${res.status}${body ? ` - ${body}` : ""}`);
}

async function fetchJson<T>(url: string, params?: Record<string, string>): Promise<T> {
  const searchParams = new URLSearchParams(params);
  const fullUrl = params ? `${url}?${searchParams}` : url;
  const res = await fetch(fullUrl);
  await throwIfNotOk(res);
  return res.json();
}

async function mutateJson<T>(url: string, init: RequestInit): Promise<T> {
  const res = await fetch(url, {
    ...init,
    headers: { "Content-Type": "application/json", ...init.headers },
  });
  await throwIfNotOk(res);
  return res.json();
}

async function mutateVoid(url: string, init: RequestInit): Promise<void> {
  const res = await fetch(url, init);
  await throwIfNotOk(res);
}

// -- Stats --------------------------------------------------------------------

export async function getSummary(
  start?: string,
  end?: string,
  groupBy = "model"
): Promise<StatsSummary> {
  const params: Record<string, string> = { group_by: groupBy };
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/stats/summary`, params);
}

export async function getTimeseries(
  metric = "cost",
  interval = "1h",
  start?: string,
  end?: string
): Promise<TimeseriesResponse> {
  const params: Record<string, string> = { metric, interval };
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/stats/timeseries`, params);
}

export async function getTopTraces(
  sortBy = "cost",
  limit = 10,
  start?: string,
  end?: string
): Promise<{ traces: TopTrace[] }> {
  const params: Record<string, string> = { sort_by: sortBy, limit: String(limit) };
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/stats/top-traces`, params);
}

export async function getWasteScore(
  start?: string,
  end?: string
): Promise<WasteScore> {
  const params: Record<string, string> = {};
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/stats/waste-score`, params);
}

export async function getEvents(
  params: Record<string, string> = {}
): Promise<EventsResponse> {
  return fetchJson(`${API_BASE}/events`, params);
}

// -- Alerts -------------------------------------------------------------------

export async function getAlertRules(): Promise<AlertRule[]> {
  return fetchJson(`${API_BASE}/alerts/rules`);
}

export async function createAlertRule(body: AlertRuleCreate): Promise<AlertRule> {
  return mutateJson(`${API_BASE}/alerts/rules`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function deleteAlertRule(ruleId: string): Promise<void> {
  return mutateVoid(`${API_BASE}/alerts/rules/${ruleId}`, { method: "DELETE" });
}

export async function getAlertHistory(
  limit = 50,
  offset = 0
): Promise<AlertHistoryResponse> {
  return fetchJson(`${API_BASE}/alerts/history`, {
    limit: String(limit),
    offset: String(offset),
  });
}

export async function getBudgets(): Promise<Budget[]> {
  return fetchJson(`${API_BASE}/budgets`);
}

export async function createBudget(body: BudgetCreate): Promise<Budget> {
  return mutateJson(`${API_BASE}/budgets`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

// -- Benchmarks ---------------------------------------------------------------

export async function getFitnessMatrix(
  start?: string,
  end?: string
): Promise<FitnessMatrixResponse> {
  const params: Record<string, string> = {};
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/benchmarks/fitness-matrix`, params);
}

export async function getBenchmarkResults(
  params: Record<string, string> = {}
): Promise<BenchmarkResultsResponse> {
  return fetchJson(`${API_BASE}/benchmarks/results`, params);
}

export async function getBenchmarkConfig(): Promise<BenchmarkConfig> {
  return fetchJson(`${API_BASE}/benchmarks/config`);
}

export async function getBenchmarkDrift(
  start?: string,
  end?: string
): Promise<DriftResponse> {
  const params: Record<string, string> = {};
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/benchmarks/drift`, params);
}

// -- Routing ------------------------------------------------------------------

export async function getRoutingPolicy(): Promise<PolicyResponse> {
  return fetchJson(`${API_BASE}/routing/policy`);
}

export async function getRoutingDecisions(
  limit = 50,
  offset = 0
): Promise<DecisionsResponse> {
  return fetchJson(`${API_BASE}/routing/decisions`, {
    limit: String(limit),
    offset: String(offset),
  });
}

export async function toggleRouting(enabled: boolean): Promise<{ routing_enabled: boolean }> {
  return mutateJson(`${API_BASE}/routing/toggle`, {
    method: "POST",
    body: JSON.stringify({ enabled }),
  });
}

export async function postDryRun(
  start: string,
  end: string,
  limit = 100
): Promise<DryRunReport> {
  return mutateJson(`${API_BASE}/routing/dry-run`, {
    method: "POST",
    body: JSON.stringify({ start, end, limit }),
  });
}

// -- Events (single) ----------------------------------------------------------

export async function getEventById(id: string): Promise<LLMEvent> {
  return fetchJson(`${API_BASE}/events/${id}`);
}

// -- MCP Tracing --------------------------------------------------------------

export async function getMCPServers(
  start?: string,
  end?: string
): Promise<MCPServersResponse> {
  const params: Record<string, string> = {};
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/mcp/stats`, params);
}

export async function getMCPGraph(traceId: string): Promise<MCPGraphResponse> {
  return fetchJson(`${API_BASE}/mcp/graph/${traceId}`);
}

export async function getMCPWaste(
  start?: string,
  end?: string
): Promise<MCPWasteResponse> {
  const params: Record<string, string> = {};
  if (start) params.start = start;
  if (end) params.end = end;
  return fetchJson(`${API_BASE}/mcp/waste`, params);
}

