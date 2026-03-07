import { InfoTip } from "./InfoTip";

interface LabeledValueProps {
  label: string;
  value: string;
  className?: string;
  tip?: string;
}

/** Reusable label + value pair with optional InfoTip. Used in Attestations, Benchmarks config, etc. */
export function LabeledValue({ label, value, className = "text-gray-200", tip }: LabeledValueProps) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-[10px] text-gray-500 uppercase flex items-center gap-1">
        {label}
        <InfoTip text={tip} />
      </span>
      <span className={`text-xs font-mono ${className}`}>{value}</span>
    </div>
  );
}
