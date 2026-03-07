import type { ReactNode } from "react";

interface CardShellProps {
  title?: ReactNode;
  loading?: boolean;
  error?: Error | null;
  /** Skeleton height when loading, e.g. "h-48" */
  skeletonHeight?: string;
  children: ReactNode;
  className?: string;
}

/** Consistent card wrapper used by every chart/widget on the dashboard. */
export function CardShell({
  title,
  loading,
  error,
  skeletonHeight = "h-48",
  children,
  className = "",
}: CardShellProps) {
  return (
    <div
      className={`glass-card rounded-lg p-4 flex flex-col ${className}`}
    >
      {title && (
        <h2 className="text-[0.625rem] font-semibold uppercase tracking-[0.15em] text-[var(--text-secondary)] mb-3">
          {title}
        </h2>
      )}

      {loading && (
        <div className={`${skeletonHeight} w-full rounded bg-white/[0.03] relative overflow-hidden`}>
          {/* Chart-shaped skeleton hint */}
          <div className="absolute inset-0 flex items-end px-4 pb-3 gap-1 opacity-20">
            <div className="w-full h-px bg-gray-600 absolute bottom-3 left-4 right-4" />
            <div className="w-px h-full bg-gray-600 absolute left-4 top-2 bottom-3" />
          </div>
          {/* Scan-line sweep */}
          <div className="absolute inset-0 overflow-hidden">
            <div className="animate-scan-line w-1/3 h-full bg-gradient-to-r from-transparent via-violet-500/10 to-transparent" />
          </div>
        </div>
      )}

      {!loading && error && (
        <div className="flex flex-col items-center justify-center gap-2 py-8">
          <svg
            className="w-6 h-6 text-red-400/60"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={1.5}
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126z"
            />
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 15.75h.007v.008H12v-.008z" />
          </svg>
          <p className="text-red-400 text-xs">{error.message}</p>
          <p className="text-[var(--text-muted)] text-[10px]">Check that the API server is running on :8100</p>
        </div>
      )}

      {!loading && !error && children}
    </div>
  );
}
