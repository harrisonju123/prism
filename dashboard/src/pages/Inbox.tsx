import { useState } from "react";
import {
  AlertTriangle,
  CheckCircle,
  Clock,
  HelpCircle,
  Lightbulb,
  ShieldAlert,
  TrendingUp,
} from "lucide-react";
import { CardShell } from "../components/common/CardShell";
import {
  useDismissInbox,
  useInbox,
  useMarkInboxRead,
  type InboxEntry,
  type InboxEntryType,
} from "../hooks/useInbox";

// ---------------------------------------------------------------------------
// Type metadata
// ---------------------------------------------------------------------------

const TYPE_FILTERS: { label: string; value: InboxEntryType | "all" }[] = [
  { label: "All", value: "all" },
  { label: "Approvals", value: "approval" },
  { label: "Blocked", value: "blocked" },
  { label: "Risks", value: "risk" },
  { label: "Cost Spikes", value: "cost_spike" },
  { label: "Suggestions", value: "suggestion" },
  { label: "Completed", value: "completed" },
];

function typeIcon(t: InboxEntryType) {
  switch (t) {
    case "approval":
      return <HelpCircle className="w-4 h-4 text-violet-400" />;
    case "blocked":
      return <AlertTriangle className="w-4 h-4 text-red-400" />;
    case "risk":
      return <ShieldAlert className="w-4 h-4 text-orange-400" />;
    case "cost_spike":
      return <TrendingUp className="w-4 h-4 text-yellow-400" />;
    case "suggestion":
      return <Lightbulb className="w-4 h-4 text-blue-400" />;
    case "completed":
      return <CheckCircle className="w-4 h-4 text-green-400" />;
  }
}

function severityPill(s: InboxEntry["severity"]) {
  const base = "text-[10px] font-medium px-1.5 py-0.5 rounded";
  switch (s) {
    case "critical":
      return <span className={`${base} bg-red-500/15 text-red-400`}>critical</span>;
    case "warning":
      return <span className={`${base} bg-yellow-500/15 text-yellow-400`}>warning</span>;
    case "info":
      return <span className={`${base} bg-blue-500/15 text-blue-400`}>info</span>;
  }
}

function relativeTime(iso: string) {
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

// ---------------------------------------------------------------------------
// Inbox page
// ---------------------------------------------------------------------------

export function Inbox() {
  const [filter, setFilter] = useState<InboxEntryType | "all">("all");
  const [unreadOnly, setUnreadOnly] = useState(false);

  const { data, isLoading, error } = useInbox(
    unreadOnly || undefined,
    filter === "all" ? undefined : filter,
  );

  const unreadCount = data?.entries.filter((e) => !e.read).length ?? 0;

  return (
    <div className="flex flex-col gap-6 max-w-[1200px] mx-auto">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-lg font-semibold">Inbox</h1>
          {unreadCount > 0 && (
            <span className="text-xs bg-violet-600/30 text-violet-300 px-2 py-0.5 rounded-full font-medium">
              {unreadCount} unread
            </span>
          )}
        </div>
        <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer">
          <input
            type="checkbox"
            checked={unreadOnly}
            onChange={(e) => setUnreadOnly(e.target.checked)}
            className="accent-violet-500"
          />
          Unread only
        </label>
      </div>

      {/* Filter bar */}
      <div className="flex gap-1 flex-wrap">
        {TYPE_FILTERS.map(({ label, value }) => (
          <button
            key={value}
            onClick={() => setFilter(value)}
            className={`px-3 py-1 text-xs rounded transition-colors ${
              filter === value
                ? "bg-violet-600/25 text-violet-300 ring-1 ring-violet-500/40"
                : "bg-white/5 text-gray-400 hover:bg-white/10"
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Entries */}
      <CardShell loading={isLoading} error={error ?? null} skeletonHeight="h-64">
        {data?.entries.length === 0 && (
          <div className="flex flex-col items-center gap-2 py-16 text-gray-500">
            <CheckCircle className="w-8 h-8 text-gray-600" />
            <p className="text-sm">
              {unreadOnly ? "No unread entries" : "Nothing here yet"}
            </p>
            <p className="text-xs text-gray-600">
              Agent events like cost spikes, approvals, and completions will appear here.
            </p>
          </div>
        )}

        <div className="flex flex-col gap-2">
          {data?.entries.map((entry) => (
            <InboxRow key={entry.id} entry={entry} />
          ))}
        </div>
      </CardShell>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Single inbox row
// ---------------------------------------------------------------------------

function InboxRow({ entry }: { entry: InboxEntry }) {
  const markRead = useMarkInboxRead();
  const dismiss = useDismissInbox();

  return (
    <div
      className={`flex items-start gap-3 px-3 py-2.5 rounded transition-colors ${
        entry.read ? "bg-gray-800/20 opacity-70" : "bg-gray-800/50"
      }`}
    >
      {/* Type icon */}
      <div className="mt-0.5 shrink-0">{typeIcon(entry.entry_type)}</div>

      {/* Content */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-sm font-medium text-gray-100 truncate">{entry.title}</span>
          {severityPill(entry.severity)}
          {!entry.read && (
            <span className="w-1.5 h-1.5 rounded-full bg-violet-400 shrink-0" />
          )}
        </div>
        {entry.body && (
          <p className="text-xs text-gray-400 mt-0.5 line-clamp-2">{entry.body}</p>
        )}
        <div className="flex items-center gap-3 mt-1 text-[10px] text-gray-500">
          {entry.source_agent && <span>from {entry.source_agent}</span>}
          <span className="flex items-center gap-1">
            <Clock className="w-3 h-3" />
            {relativeTime(entry.created_at)}
          </span>
          {entry.ref_type && entry.ref_id && (
            <span className="font-mono text-gray-600">
              {entry.ref_type}:{entry.ref_id.slice(0, 8)}
            </span>
          )}
        </div>
      </div>

      {/* Actions */}
      <div className="flex gap-1 shrink-0">
        {!entry.read && (
          <button
            onClick={() => markRead.mutate(entry.id)}
            disabled={markRead.isPending}
            className="text-[10px] text-gray-500 hover:text-gray-300 transition-colors px-2 py-1 rounded hover:bg-white/5 disabled:opacity-40"
          >
            Mark read
          </button>
        )}
        <button
          onClick={() => dismiss.mutate(entry.id)}
          disabled={dismiss.isPending}
          className="text-[10px] text-gray-500 hover:text-red-400 transition-colors px-2 py-1 rounded hover:bg-white/5 disabled:opacity-40"
        >
          Dismiss
        </button>
      </div>
    </div>
  );
}
