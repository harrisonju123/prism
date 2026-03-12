import { useEffect, type ReactNode } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { motion } from "framer-motion";
import {
  BarChart3,
  List,
  Bell,
  FlaskConical,
  Inbox,
  Scale,
  Route,
  Plug,
  Bug,
} from "lucide-react";
import type { TimeRange } from "../../hooks/useStats";
import { LiveIndicator } from "../common/LiveIndicator";
import { useUnreadInboxCount } from "../../hooks/useInbox";

interface ShellProps {
  timeRange: TimeRange;
  onTimeRangeChange: (range: TimeRange) => void;
  children: ReactNode;
}

const RANGES: { label: string; value: TimeRange; shortcut: string }[] = [
  { label: "24h", value: "24h", shortcut: "1" },
  { label: "7d", value: "7d", shortcut: "2" },
  { label: "30d", value: "30d", shortcut: "3" },
];

const NAV_ITEMS = [
  { to: "/", label: "Overview", icon: BarChart3 },
  { to: "/inbox", label: "Inbox", icon: Inbox },
  { to: "/events", label: "Events", icon: List },
  { to: "/alerts", label: "Alerts", icon: Bell },
  { to: "/benchmarks", label: "Benchmarks", icon: FlaskConical },
  { to: "/waste", label: "Waste", icon: Scale },
  { to: "/routing", label: "Routing", icon: Route },
  { to: "/mcp", label: "MCP Tracing", icon: Plug },
  { to: "/debugging", label: "Debugging", icon: Bug },
] as const;

export function Shell({ timeRange, onTimeRangeChange, children }: ShellProps) {
  const location = useLocation();
  const inboxUnread = useUnreadInboxCount();

  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement
      )
        return;
      const match = RANGES.find((r) => r.shortcut === e.key);
      if (match) onTimeRangeChange(match.value);
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onTimeRangeChange]);

  useEffect(() => {
    const current = NAV_ITEMS.find((item) =>
      item.to === "/"
        ? location.pathname === "/"
        : location.pathname.startsWith(item.to),
    );
    const page = current?.label ?? "PrisM";
    document.title = `${page} [${timeRange}] — PrisM`;
  }, [timeRange, location.pathname]);

  return (
    <div className="min-h-screen bg-[#020409] text-[var(--text-primary)] flex">
      {/* Sidebar */}
      <aside
        className="w-52 shrink-0 rounded-none flex flex-col relative"
        style={{
          background: "var(--glass-bg)",
          backdropFilter: "blur(var(--glass-blur))",
          WebkitBackdropFilter: "blur(var(--glass-blur))",
          borderRight: "1px solid var(--glass-border)",
        }}
      >
        {/* Gradient accent on right edge */}
        <div className="absolute right-0 top-0 bottom-0 w-px bg-gradient-to-b from-transparent via-violet-500/30 to-transparent pointer-events-none" />

        {/* Logo */}
        <div className="px-4 py-4 flex items-center gap-2 border-b border-white/[0.06]">
          <svg
            width="16"
            height="16"
            viewBox="0 0 16 16"
            fill="none"
            className="shrink-0"
            style={{ filter: "drop-shadow(0 0 6px rgba(139, 92, 246, 0.5))" }}
            aria-hidden="true"
          >
            <path
              d="M8 1L3 4v4.5c0 3.5 2.5 5.5 5 6.5 2.5-1 5-3 5-6.5V4L8 1z"
              fill="#8b5cf6"
              stroke="#7c3aed"
              strokeWidth="0.5"
            />
            <path
              d="M8 5v4M6 7h4"
              stroke="white"
              strokeWidth="1.2"
              strokeLinecap="round"
            />
          </svg>
          <span className="text-sm font-semibold tracking-widest uppercase text-[var(--text-hero)]">
            PrisM
          </span>
        </div>

        {/* Navigation */}
        <nav className="relative flex-1 px-2 py-3 flex flex-col gap-0.5">
          {NAV_ITEMS.map(({ to, label, icon: Icon }) => {
            const isActive =
              to === "/"
                ? location.pathname === "/"
                : location.pathname.startsWith(to);

            return (
              <NavLink
                key={to}
                to={to}
                end={to === "/"}
                className="relative flex items-center gap-2.5 px-3 py-2 rounded-md text-sm transition-colors z-10"
                style={{
                  color: isActive ? "#c4b5fd" : "#94a3b8",
                }}
              >
                {isActive && (
                  <motion.div
                    layoutId="nav-active"
                    className="absolute inset-0 rounded-md"
                    style={{
                      background: "rgba(139, 92, 246, 0.12)",
                      boxShadow:
                        "0 0 12px rgba(139, 92, 246, 0.15), inset 0 0 0 1px rgba(139, 92, 246, 0.2)",
                    }}
                    transition={{
                      type: "spring",
                      stiffness: 380,
                      damping: 30,
                    }}
                  />
                )}
                <Icon className="w-4 h-4 shrink-0 relative z-10" />
                <span className="relative z-10">{label}</span>
                {to === "/inbox" && inboxUnread > 0 && (
                  <span className="relative z-10 ml-auto text-[10px] bg-violet-600/40 text-violet-300 px-1.5 py-0.5 rounded-full font-medium leading-none">
                    {inboxUnread}
                  </span>
                )}
              </NavLink>
            );
          })}
        </nav>

        {/* Footer */}
        <div className="px-4 py-3 border-t border-white/[0.06]">
          <LiveIndicator />
        </div>
      </aside>

      {/* Main content */}
      <div className="flex-1 flex flex-col min-w-0">
        {/* Top bar */}
        <header className="border-b border-white/[0.06] bg-[var(--glass-bg)] backdrop-blur-sm px-4 sm:px-6 py-3 flex items-center justify-end shrink-0">
          <nav aria-label="Time range" className="flex gap-1">
            {RANGES.map(({ label, value, shortcut }) => {
              const active = timeRange === value;
              return (
                <button
                  key={value}
                  onClick={() => onTimeRangeChange(value)}
                  aria-pressed={active}
                  title={`Switch to ${label} (${shortcut})`}
                  className="relative px-3 py-1 text-xs font-mono rounded transition-colors"
                  style={{
                    color: active ? "#c4b5fd" : "#94a3b8",
                  }}
                >
                  {active && (
                    <motion.div
                      layoutId="time-range-active"
                      className="absolute inset-0 rounded"
                      style={{
                        background: "rgba(139, 92, 246, 0.15)",
                        boxShadow:
                          "0 0 10px rgba(139, 92, 246, 0.12), inset 0 0 0 1px rgba(139, 92, 246, 0.25)",
                      }}
                      transition={{
                        type: "spring",
                        stiffness: 400,
                        damping: 28,
                      }}
                    />
                  )}
                  <span className="relative z-10">{label}</span>
                </button>
              );
            })}
          </nav>
        </header>

        <main className="flex-1 px-4 sm:px-6 py-6 overflow-auto">
          {children}
        </main>
      </div>
    </div>
  );
}
