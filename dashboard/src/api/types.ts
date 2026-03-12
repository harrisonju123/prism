export interface StatsSummary {
  period: { start: string; end: string };
  total_requests: number;
  total_cost_usd: number;
  total_tokens: number;
  failure_rate: number;
  groups: StatGroup[];
}

export interface StatGroup {
  key: string;
  request_count: number;
  total_cost_usd: number;
  avg_latency_ms: number;
  p95_latency_ms: number;
  avg_cost_per_request_usd: number;
  total_prompt_tokens: number;
  total_completion_tokens: number;
  failure_count: number;
}

export interface TimeseriesResponse {
  metric: "cost" | "requests" | "latency" | "tokens";
  interval: "1h" | "6h" | "1d";
  data: TimeseriesPoint[];
}

export interface TimeseriesPoint {
  timestamp: string;
  value: number;
}

export interface TopTrace {
  trace_id: string;
  total_cost_usd: number;
  total_tokens: number;
  total_latency_ms: number;
  event_count: number;
  models_used: string[];
  first_event_at: string;
  last_event_at: string;
  agent_framework: string | null;
}

export interface WasteScore {
  waste_score: number;
  total_potential_savings_usd: number;
  breakdown: WasteBreakdownItem[];
}

export interface WasteBreakdownItem {
  task_type: string;
  current_model: string;
  suggested_model: string;
  call_count: number;
  current_cost_usd: number;
  projected_cost_usd: number;
  savings_usd: number;
  confidence: number;
  suggestion_source?: "fitness" | "heuristic" | null;
  quality_score?: number | null;
  sample_size?: number | null;
}

export interface EventsResponse {
  events: LLMEvent[];
  total_count: number;
  has_more: boolean;
}

export interface LLMEvent {
  id: string;
  created_at: string;
  status: "success" | "failure";
  provider: string;
  model: string;
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
  estimated_cost: number;
  latency_ms: number;
  trace_id: string;
  span_id: string;
  task_type: string | null;
  task_type_confidence: number | null;
  has_tool_calls: boolean;
  agent_framework: string | null;
}

// -- Alert types --------------------------------------------------------------

export type RuleType =
  | "spend_threshold"
  | "anomaly_zscore"
  | "error_rate"
  | "latency_p95";
export type AlertChannel = "slack" | "email" | "both";
export type AlertSeverity = "info" | "warning" | "critical";
export type BudgetPeriod = "daily" | "weekly" | "monthly";
export type BudgetAction = "alert" | "downgrade" | "block";

export interface AlertRule {
  id: string;
  org_id: string;
  rule_type: RuleType;
  threshold_config: Record<string, unknown>;
  channel: AlertChannel;
  webhook_url: string | null;
  enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface AlertRuleCreate {
  org_id: string;
  rule_type: RuleType;
  threshold_config: Record<string, unknown>;
  channel: AlertChannel;
  webhook_url?: string | null;
  enabled?: boolean;
}

export interface AlertHistoryItem {
  id: string;
  rule_id: string;
  triggered_at: string;
  message: string;
  severity: AlertSeverity;
  resolved: boolean;
  resolved_at: string | null;
}

export interface AlertHistoryResponse {
  items: AlertHistoryItem[];
  total_count: number;
  has_more: boolean;
}

export interface Budget {
  id: string;
  org_id: string;
  project_id: string | null;
  budget_usd: number;
  period: BudgetPeriod;
  action: BudgetAction;
  current_spend: number;
  period_start: string;
  created_at: string;
}

export interface BudgetCreate {
  org_id: string;
  project_id?: string | null;
  budget_usd: number;
  period: BudgetPeriod;
  action: BudgetAction;
}

// -- Benchmarking types -------------------------------------------------------

export interface FitnessEntry {
  task_type: string;
  model: string;
  avg_quality: number;
  avg_cost: number;
  avg_latency: number;
  sample_size: number;
}

export interface FitnessMatrixResponse {
  entries: FitnessEntry[];
}

export interface BenchmarkResult {
  id: string;
  created_at: string;
  original_event_id: string;
  original_model: string;
  benchmark_model: string;
  task_type: string;
  quality_score: number;
  original_cost: number;
  benchmark_cost: number;
  original_latency_ms: number;
  benchmark_latency_ms: number;
  judge_model: string;
  rubric_version: string;
  org_id: string | null;
}

export interface BenchmarkResultsResponse {
  results: BenchmarkResult[];
  total_count: number;
  has_more: boolean;
}

export interface BenchmarkConfig {
  enabled: boolean;
  sample_rate: number;
  benchmark_models: string[];
  judge_model: string;
  enabled_task_types: string[];
}

export interface BenchmarkConfigUpdate {
  enabled?: boolean;
  sample_rate?: number;
  benchmark_models?: string[];
  judge_model?: string;
  enabled_task_types?: string[];
}

export interface DriftEntry {
  model: string;
  task_type: string;
  baseline_quality: number;
  current_quality: number;
  delta_pct: number;
  p_value: number;
  confidence_interval: [number, number];
  baseline_sample_size: number;
  current_sample_size: number;
  first_detected_at: string;
}

export interface DriftResponse {
  drifts: DriftEntry[];
  models_checked: number;
  drifts_found: number;
}

// -- Routing types ------------------------------------------------------------

export interface RoutingRule {
  task_type: string;
  criteria: string;
  min_quality: number;
  max_cost_per_1k: number | null;
  max_latency_ms: number | null;
  fallback: string;
}

export interface RoutingPolicy {
  rules: RoutingRule[];
  version: number;
}

export interface PolicyResponse {
  policy: RoutingPolicy;
  is_default: boolean;
  routing_enabled: boolean;
}

export interface RoutingDecision {
  selected_model: string;
  reason: string;
  was_overridden: boolean;
  policy_version: number | null;
  group_name: string | null;
}

export interface DecisionsResponse {
  decisions: RoutingDecision[];
  total_count: number;
}

export interface DebugSessionSummary {
  id: string;
  title: string;
  status: string;
  symptom: Record<string, unknown>;
  metadata: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface DebugHypothesis {
  id: string;
  session_id: string;
  rank: number;
  statement: string;
  confidence: number;
  evidence: unknown;
  status: string;
  created_at: string;
  updated_at: string;
}

export interface DebugExperiment {
  id: string;
  session_id: string;
  hypothesis_id: string | null;
  title: string;
  description: string | null;
  cost_level: string;
  impact_level: string;
  status: string;
  params: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface DebugRun {
  id: string;
  experiment_id: string;
  status: string;
  started_at: string | null;
  finished_at: string | null;
  duration_ms: number | null;
  output: string | null;
  artifacts: Record<string, unknown>;
  created_at: string;
}

export interface DebugSessionDetail {
  session: DebugSessionSummary;
  hypotheses: DebugHypothesis[];
  experiments: DebugExperiment[];
  runs: DebugRun[];
}

export interface CreateDebugSessionRequest {
  title: string;
  symptom?: Record<string, unknown>;
  metadata?: Record<string, unknown>;
}

export interface CreateHypothesisRequest {
  rank?: number;
  statement: string;
  confidence?: number;
  evidence?: unknown;
  status?: string;
}

export interface CreateExperimentRequest {
  hypothesis_id?: string | null;
  title: string;
  description?: string | null;
  cost_level?: string;
  impact_level?: string;
  status?: string;
  params?: Record<string, unknown>;
}

export interface CreateRunRequest {
  status?: string;
  started_at?: string | null;
  finished_at?: string | null;
  duration_ms?: number | null;
  output?: string | null;
  artifacts?: Record<string, unknown>;
}

export type DryRunReport = Record<string, unknown>;

// -- MCP types ----------------------------------------------------------------

export interface MCPServer {
  server_name: string;
  call_count: number;
  failure_count: number;
  failure_rate: number;
  avg_latency_ms: number | null;
  p50_latency_ms: number | null;
  p95_latency_ms: number | null;
}

export interface MCPServersResponse {
  stats: MCPServer[];
}

export interface MCPCall {
  id: string;
  event_id: string;
  created_at: string;
  server_name: string;
  method: string;
  params_hash: string;
  response_hash: string | null;
  latency_ms: number | null;
  response_tokens: number | null;
  status: "success" | "failure";
  error_type: string | null;
}

export interface MCPGraphNode {
  id: string;
  event_id: string;
  created_at: string;
  server_name: string;
  method: string;
  params_hash: string;
  response_hash: string | null;
  latency_ms: number | null;
  response_tokens: number | null;
  status: string;
  error_type: string | null;
}

export interface MCPGraphEdge {
  id: string;
  parent_call_id: string;
  child_call_id: string;
  trace_id: string;
}

export interface MCPGraphResponse {
  trace_id: string;
  nodes: MCPGraphNode[];
  edges: MCPGraphEdge[];
}

export interface MCPWasteItem {
  server_name: string;
  method: string;
  unused_call_count: number;
  total_wasted_tokens: number;
  avg_wasted_tokens: number;
}

export interface MCPWasteResponse {
  waste: MCPWasteItem[];
}
