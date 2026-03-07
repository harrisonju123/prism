import { useMemo } from "react";
import type { TimeRange } from "../hooks/useStats";
import { useWasteScore } from "../hooks/useStats";
import { CardShell } from "../components/common/CardShell";
import { InfoTip } from "../components/common/InfoTip";
import { formatUSD } from "../utils/format";

interface Props {
  timeRange: TimeRange;
}

export function WasteDetails({ timeRange }: Props) {
  const { data, isLoading, error } = useWasteScore(timeRange);

  // Group breakdown by task type for summary
  const taskSummary = useMemo(() => {
    if (!data?.breakdown) return [];
    const byTask = new Map<string, { calls: number; savings: number; current: number }>();
    for (const item of data.breakdown) {
      const key = item.task_type;
      const existing = byTask.get(key) ?? { calls: 0, savings: 0, current: 0 };
      existing.calls += item.call_count;
      existing.savings += item.savings_usd;
      existing.current += item.current_cost_usd;
      byTask.set(key, existing);
    }
    return [...byTask.entries()]
      .map(([task, stats]) => ({ task, ...stats }))
      .sort((a, b) => b.savings - a.savings);
  }, [data?.breakdown]);

  const totalSavings = data?.total_potential_savings_usd ?? 0;

  return (
    <div className="flex flex-col gap-6 max-w-[1600px] mx-auto">
      <h1 className="text-lg font-semibold flex items-center gap-2">Waste Analysis <InfoTip text="Identifies LLM calls where a cheaper model could produce equivalent results. Savings estimates use benchmark quality data." /></h1>

      {/* Summary bar */}
      <div className="flex flex-wrap gap-6 bg-gray-800/50 rounded-lg px-4 py-3">
        <div className="flex flex-col gap-0.5">
          <span className="text-[10px] text-gray-500 uppercase flex items-center gap-1">Waste Score <InfoTip text="0–100 score weighted by cost. 0 = no detectable waste, 100 = every call could be downgraded." /></span>
          <span className={`text-2xl font-mono font-bold ${
            (data?.waste_score ?? 0) > 50 ? "text-red-400"
              : (data?.waste_score ?? 0) > 25 ? "text-yellow-400"
              : "text-green-400"
          }`}>
            {data?.waste_score?.toFixed(0) ?? "—"}
          </span>
        </div>
        <div className="flex flex-col gap-0.5">
          <span className="text-[10px] text-gray-500 uppercase flex items-center gap-1">Potential Savings <InfoTip text="Estimated savings if all suggested model swaps were applied, assuming quality stays above fitness threshold." /></span>
          <span className="text-2xl font-mono font-bold text-green-400">
            {formatUSD(totalSavings)}
          </span>
        </div>
      </div>

      {/* Per-task-type savings */}
      <CardShell title="Savings by Task Type" loading={isLoading} error={error ?? null} skeletonHeight="h-48">
        {taskSummary.length === 0 && (
          <p className="text-xs text-gray-500 py-4 text-center">No waste data available</p>
        )}
        <div className="flex flex-col gap-2">
          {taskSummary.map(({ task, calls, savings, current }) => {
            const savingsPct = current > 0 ? (savings / current) * 100 : 0;
            return (
              <div key={task} className="flex items-center gap-3">
                <span className="text-xs font-mono text-gray-200 w-32 shrink-0">{task}</span>
                <div className="flex-1 h-2 bg-gray-700 rounded-full overflow-hidden">
                  <div
                    className="h-full bg-green-500 rounded-full"
                    style={{ width: `${Math.min(savingsPct, 100)}%` }}
                  />
                </div>
                <span className="text-xs font-mono text-green-400 w-20 text-right">{formatUSD(savings)}</span>
                <span className="text-[10px] text-gray-500 w-16 text-right">{calls} calls</span>
              </div>
            );
          })}
        </div>
      </CardShell>

      {/* Detailed breakdown table */}
      <CardShell title="Model-Level Breakdown" loading={isLoading} error={error ?? null} skeletonHeight="h-64">
        {(!data?.breakdown || data.breakdown.length === 0) && (
          <p className="text-xs text-gray-500 py-4 text-center">No waste data available</p>
        )}
        {data?.breakdown && data.breakdown.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="text-gray-500 border-b border-gray-800">
                  <th className="text-left py-2 px-2 font-medium">Task Type</th>
                  <th className="text-left py-2 px-2 font-medium">Current Model</th>
                  <th className="text-left py-2 px-2 font-medium">Suggested</th>
                  <th className="text-right py-2 px-2 font-medium">Calls</th>
                  <th className="text-right py-2 px-2 font-medium">Current Cost</th>
                  <th className="text-right py-2 px-2 font-medium">Projected</th>
                  <th className="text-right py-2 px-2 font-medium">Savings</th>
                  <th className="text-right py-2 px-2 font-medium">Confidence</th>
                </tr>
              </thead>
              <tbody>
                {data.breakdown
                  .sort((a, b) => b.savings_usd - a.savings_usd)
                  .map((item, i) => (
                    <tr key={i} className="border-b border-gray-800/50 hover:bg-gray-800/30 transition-colors">
                      <td className="py-2 px-2 text-gray-300">{item.task_type}</td>
                      <td className="py-2 px-2 font-mono text-gray-300">{item.current_model}</td>
                      <td className="py-2 px-2 font-mono text-violet-300">{item.suggested_model}</td>
                      <td className="py-2 px-2 text-right text-gray-300">{item.call_count.toLocaleString()}</td>
                      <td className="py-2 px-2 text-right font-mono text-gray-300">{formatUSD(item.current_cost_usd)}</td>
                      <td className="py-2 px-2 text-right font-mono text-gray-300">{formatUSD(item.projected_cost_usd)}</td>
                      <td className="py-2 px-2 text-right font-mono text-green-400">{formatUSD(item.savings_usd)}</td>
                      <td className="py-2 px-2 text-right">
                        <span className={`font-mono ${
                          item.confidence >= 0.8 ? "text-green-400"
                            : item.confidence >= 0.5 ? "text-yellow-400"
                            : "text-gray-500"
                        }`}>
                          {(item.confidence * 100).toFixed(0)}%
                        </span>
                      </td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </div>
        )}
      </CardShell>
    </div>
  );
}
