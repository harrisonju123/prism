import { useState } from "react";

export function InfoTip({ text }: { text?: string }) {
  const [open, setOpen] = useState(false);
  if (!text) return null;
  return (
    <span className="inline-flex flex-col">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        aria-label="More info"
        className="inline-flex items-center justify-center w-4 h-4 rounded-full text-gray-500 hover:text-gray-300 hover:bg-gray-700/50 transition-colors focus:outline-none focus:ring-1 focus:ring-violet-500"
      >
        <svg className="w-3 h-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
          <circle cx="8" cy="8" r="7" />
          <path d="M8 11V8M8 5.5v-.01" />
        </svg>
      </button>
      <span
        className={`overflow-hidden transition-all duration-200 text-[10px] leading-relaxed text-gray-400 ${
          open ? "max-h-40 opacity-100 mt-1" : "max-h-0 opacity-0"
        }`}
      >
        {text}
      </span>
    </span>
  );
}
