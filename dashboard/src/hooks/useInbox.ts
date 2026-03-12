import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

const API_BASE = "/api/v1";

export type InboxSeverity = "critical" | "warning" | "info";
export type InboxEntryType =
  | "approval"
  | "blocked"
  | "suggestion"
  | "risk"
  | "cost_spike"
  | "completed";

export interface InboxEntry {
  id: string;
  workspace_id: string;
  entry_type: InboxEntryType;
  title: string;
  body?: string;
  severity: InboxSeverity;
  source_agent?: string;
  ref_type?: string;
  ref_id?: string;
  read: boolean;
  dismissed: boolean;
  created_at: string;
}

export interface InboxListResponse {
  entries: InboxEntry[];
  total: number;
}

async function fetchInbox(unread?: boolean, type?: InboxEntryType): Promise<InboxListResponse> {
  const params = new URLSearchParams();
  if (unread != null) params.set("unread", String(unread));
  if (type) params.set("type", type);
  const url = `${API_BASE}/inbox${params.toString() ? `?${params}` : ""}`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`API error: ${res.status}`);
  return res.json();
}

async function patchInbox(id: string, action: "read" | "dismiss"): Promise<void> {
  const res = await fetch(`${API_BASE}/inbox/${id}/${action}`, { method: "PATCH" });
  if (!res.ok) throw new Error(`API error: ${res.status}`);
}

export function useInbox(unread?: boolean, type?: InboxEntryType) {
  return useQuery({
    queryKey: ["inbox", unread, type],
    queryFn: () => fetchInbox(unread, type),
    refetchInterval: 10_000,
    staleTime: 5_000,
  });
}

export function useUnreadInboxCount() {
  const { data } = useInbox(true);
  return data?.total ?? 0;
}

export function useMarkInboxRead() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => patchInbox(id, "read"),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["inbox"] }),
  });
}

export function useDismissInbox() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => patchInbox(id, "dismiss"),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["inbox"] }),
  });
}
