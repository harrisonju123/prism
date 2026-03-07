export function LiveIndicator({ className = "" }: { className?: string }) {
  return (
    <span className={`inline-flex items-center gap-1.5 ${className}`}>
      <span className="relative flex h-2 w-2">
        <span className="animate-pulse-live absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75" />
        <span className="relative inline-flex rounded-full h-2 w-2 bg-emerald-500" />
      </span>
      <span className="text-[0.625rem] font-semibold tracking-[0.15em] uppercase text-emerald-400">
        Live
      </span>
    </span>
  );
}
