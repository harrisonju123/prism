import { useEffect, useCallback, useRef } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { X } from "lucide-react";
import { useEvent } from "../hooks/useEvents";
import { formatUSD, formatMs } from "../utils/format";
import type { LLMEvent } from "../api/types";

interface Props {
  eventId: string | null;
  onClose: () => void;
}

export function EventDrawer({ eventId, onClose }: Props) {
  const { data: event, isLoading, error } = useEvent(eventId);
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!eventId) return;
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.body.style.overflow = "hidden";
    window.addEventListener("keydown", handleKey);
    return () => {
      document.body.style.overflow = "";
      window.removeEventListener("keydown", handleKey);
    };
  }, [eventId, onClose]);

  const handleOverlayClick = useCallback(
    (e: React.MouseEvent) => {
      if (e.target === overlayRef.current) onClose();
    },
    [onClose],
  );

  return (
    <AnimatePresence>
      {eventId && (
        <motion.div
          ref={overlayRef}
          onClick={handleOverlayClick}
          role="dialog"
          aria-modal="true"
          aria-label="Event detail"
          className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.2 }}
        >
          <motion.div
            className="absolute right-0 top-0 bottom-0 w-full max-w-lg glass-card rounded-none border-t-0 border-b-0 border-r-0 shadow-2xl flex flex-col"
            style={{ borderLeft: '1px solid var(--glass-border)' }}
            initial={{ x: "100%" }}
            animate={{ x: 0 }}
            exit={{ x: "100%" }}
            transition={{ duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
          >
            {/* Header */}
            <div className="flex items-center justify-between px-5 py-4 border-b border-white/[0.06] shrink-0">
              <h2 className="text-sm font-semibold text-[var(--text-hero)]">Event Detail</h2>
              <button
                type="button"
                onClick={onClose}
                className="p-1 rounded hover:bg-white/[0.06] transition-colors text-[var(--text-secondary)] hover:text-[var(--text-hero)]"
                aria-label="Close drawer"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            {/* Body */}
            <div className="flex-1 overflow-y-auto px-5 py-4">
              {isLoading && (
                <div className="flex flex-col gap-3">
                  {Array.from({ length: 6 }).map((_, i) => (
                    <div key={i} className="h-5 bg-white/[0.03] rounded relative overflow-hidden">
                      <div className="animate-scan-line w-1/3 h-full bg-gradient-to-r from-transparent via-violet-500/10 to-transparent" />
                    </div>
                  ))}
                </div>
              )}

              {error && (
                <div className="text-xs text-red-400 py-4 text-center">
                  {(error as Error).message}
                </div>
              )}

              {event && !isLoading && <EventBody event={event} />}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function EventBody({ event }: { event: LLMEvent }) {
  return (
    <div className="flex flex-col gap-5">
      {/* Status + Model header */}
      <div className="flex items-center gap-3">
        <span
          className={`inline-block px-2 py-0.5 rounded text-[11px] font-medium ${
            event.status === "success"
              ? "bg-emerald-500/15 text-emerald-400"
              : "bg-red-500/15 text-red-400"
          }`}
        >
          {event.status}
        </span>
        <span className="font-mono text-sm text-[var(--text-hero)]">{event.model}</span>
      </div>

      <Section title="General">
        <Row label="ID" value={event.id} mono />
        <Row label="Provider" value={event.provider} />
        <Row label="Model" value={event.model} mono />
        <Row
          label="Created"
          value={new Date(event.created_at).toLocaleString("en-US", {
            year: "numeric",
            month: "short",
            day: "numeric",
            hour: "numeric",
            minute: "2-digit",
            second: "2-digit",
          })}
        />
        <Row label="Status" value={event.status} />
      </Section>

      <Section title="Tokens">
        <Row label="Prompt" value={event.prompt_tokens.toLocaleString()} mono />
        <Row label="Completion" value={event.completion_tokens.toLocaleString()} mono />
        <Row label="Total" value={event.total_tokens.toLocaleString()} mono />
      </Section>

      <Section title="Cost & Latency">
        <Row label="Estimated Cost" value={formatUSD(event.estimated_cost)} mono />
        <Row label="Latency" value={formatMs(event.latency_ms)} mono />
      </Section>

      <Section title="Trace Context">
        <Row label="Trace ID" value={event.trace_id} mono />
        <Row label="Span ID" value={event.span_id} mono />
      </Section>

      <Section title="Classification">
        <Row label="Task Type" value={event.task_type ?? "---"} />
        <Row
          label="Confidence"
          value={
            event.task_type_confidence != null
              ? `${(event.task_type_confidence * 100).toFixed(1)}%`
              : "---"
          }
          mono
        />
      </Section>

      <Section title="Agent & Tools">
        <Row label="Agent Framework" value={event.agent_framework ?? "---"} />
        <Row label="Has Tool Calls" value={event.has_tool_calls ? "Yes" : "No"} />
      </Section>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <h3 className="text-[10px] font-medium uppercase tracking-[0.15em] text-[var(--text-muted)] mb-0.5">
        {title}
      </h3>
      {children}
    </div>
  );
}

function Row({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between gap-4">
      <span className="text-xs text-[var(--text-secondary)] shrink-0">{label}</span>
      <span
        className={`text-xs text-[var(--text-primary)] text-right break-all ${mono ? "font-mono" : ""}`}
      >
        {value}
      </span>
    </div>
  );
}
