import { CardShell } from "../components/common/CardShell";
import { InfoTip } from "../components/common/InfoTip";

const symptom = {
  title: "Replay #9831 returns empty response body",
  source: "tests/replay/payment-flow.spec.ts",
  summary:
    "The replay passes auth, but the model response payload is empty and UI renders a blank panel.",
  evidence: [
    "Latency spikes to 4.6s on the failing replay",
    "No tool calls recorded during the failed run",
    "Previous replay on same prompt returned 312 tokens",
  ],
};

const hypotheses = [
  {
    id: "h1",
    title: "Cache key misses after replay hydration",
    confidence: 0.62,
    evidence: ["Cache warm path uses request_id", "Replays set run_id only"],
  },
  {
    id: "h2",
    title: "Provider timeout leads to empty streaming buffer",
    confidence: 0.48,
    evidence: ["Timeout set to 4s", "Latency spikes right before failure"],
  },
  {
    id: "h3",
    title: "Response filtering removes all tokens on schema mismatch",
    confidence: 0.28,
    evidence: ["Schema changed in last release", "Filter runs post-stream"],
  },
];

const experiments = [
  {
    id: "e1",
    title: "Force cache bypass during replay",
    cost: "low",
    impact: "high",
    status: "ready",
    detail: "Toggle replay_cache=false and inspect response delta.",
  },
  {
    id: "e2",
    title: "Extend provider timeout to 8s",
    cost: "med",
    impact: "med",
    status: "queued",
    detail: "Compare latency + output size on the same replay.",
  },
  {
    id: "e3",
    title: "Disable response filter on replay",
    cost: "low",
    impact: "high",
    status: "ready",
    detail: "Skip filter pass, capture raw provider payload.",
  },
];

const runLog = {
  command: "prism replay --id 9831 --cache-bypass",
  duration: "5.1s",
  status: "success",
  output: [
    "Replay started (trace_id=tr_98a1)",
    "Cache bypass enabled",
    "Provider response: 312 tokens",
    "UI render restored",
  ],
};

const beliefState = {
  current: "Cache key misses after replay hydration",
  confidence: 0.74,
  nextAction: "Ship cache key fix + replay regression",
  timeline: [
    { label: "Symptom captured", state: "done" },
    { label: "Hypotheses ranked", state: "done" },
    { label: "Experiment run", state: "done" },
    { label: "Belief updated", state: "current" },
    { label: "Next action queued", state: "next" },
  ],
};

const costBadge = (cost: string) => {
  if (cost === "low") return "bg-emerald-500/15 text-emerald-300";
  if (cost === "med") return "bg-amber-500/15 text-amber-300";
  return "bg-red-500/15 text-red-300";
};

const impactBadge = (impact: string) => {
  if (impact === "high") return "bg-violet-500/15 text-violet-300";
  if (impact === "med") return "bg-blue-500/15 text-blue-300";
  return "bg-gray-500/15 text-gray-300";
};

export function Debugging() {
  return (
    <div className="flex flex-col gap-6 max-w-[1600px] mx-auto">
      <div className="flex items-center gap-2">
        <h1 className="text-lg font-semibold">IDE Debugging: Hypothesis Loop</h1>
        <InfoTip text="Move from step-through debugging to an agentic loop with ranked hypotheses and experiments." />
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <CardShell title="1. Symptom">
          <div className="flex flex-col gap-3">
            <div>
              <p className="text-sm text-gray-200 font-medium">{symptom.title}</p>
              <p className="text-[11px] text-gray-500">{symptom.source}</p>
            </div>
            <p className="text-xs text-gray-300">{symptom.summary}</p>
            <div className="flex flex-col gap-1">
              {symptom.evidence.map((item) => (
                <span key={item} className="text-[11px] text-gray-400">
                  • {item}
                </span>
              ))}
            </div>
          </div>
        </CardShell>

        <CardShell title="2. Hypotheses (ranked)">
          <div className="flex flex-col gap-3">
            {hypotheses.map((hyp, index) => (
              <div key={hyp.id} className="bg-gray-800/40 rounded px-3 py-2">
                <div className="flex items-center gap-2">
                  <span className="text-[10px] uppercase tracking-widest text-gray-500">
                    #{index + 1}
                  </span>
                  <span className="text-xs text-gray-200">{hyp.title}</span>
                  <span className="ml-auto text-[10px] text-gray-500">
                    {(hyp.confidence * 100).toFixed(0)}% confidence
                  </span>
                </div>
                <div className="mt-2 h-1.5 bg-gray-700 rounded-full overflow-hidden">
                  <div
                    className="h-full bg-violet-500"
                    style={{ width: `${hyp.confidence * 100}%` }}
                  />
                </div>
                <div className="mt-2 flex flex-wrap gap-2 text-[10px] text-gray-500">
                  {hyp.evidence.map((item) => (
                    <span key={item} className="px-1.5 py-0.5 bg-gray-700/40 rounded">
                      {item}
                    </span>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </CardShell>
      </div>

      <CardShell title={<span className="flex items-center gap-1">3. Proposed Experiments <InfoTip text="Ranked by cost/impact and ready to execute." /></span>}>
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-3">
          {experiments.map((exp) => (
            <div key={exp.id} className="bg-gray-800/40 rounded p-3 flex flex-col gap-2">
              <div className="flex items-start justify-between gap-2">
                <span className="text-xs text-gray-200">{exp.title}</span>
                <span className="text-[10px] uppercase text-gray-500">{exp.status}</span>
              </div>
              <p className="text-[11px] text-gray-500">{exp.detail}</p>
              <div className="flex items-center gap-2">
                <span className={`text-[10px] px-1.5 py-0.5 rounded ${costBadge(exp.cost)}`}>
                  cost: {exp.cost}
                </span>
                <span className={`text-[10px] px-1.5 py-0.5 rounded ${impactBadge(exp.impact)}`}>
                  impact: {exp.impact}
                </span>
              </div>
              <button className="mt-auto text-[11px] text-violet-300 bg-violet-500/10 rounded px-2 py-1 hover:bg-violet-500/20 transition-colors">
                Run experiment
              </button>
            </div>
          ))}
        </div>
      </CardShell>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <CardShell title="4. Run Experiment">
          <div className="flex flex-col gap-3">
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-200 font-mono">{runLog.command}</span>
              <span className="text-[10px] text-emerald-300 bg-emerald-500/10 px-2 py-0.5 rounded">
                {runLog.status}
              </span>
            </div>
            <span className="text-[11px] text-gray-500">Duration: {runLog.duration}</span>
            <div className="bg-gray-900/60 rounded p-3 text-[11px] text-gray-300 font-mono space-y-1">
              {runLog.output.map((line) => (
                <div key={line}>{line}</div>
              ))}
            </div>
          </div>
        </CardShell>

        <CardShell title="5. Update Belief State + Next Action">
          <div className="flex flex-col gap-3">
            <div>
              <p className="text-xs text-gray-400 uppercase tracking-widest">Current belief</p>
              <p className="text-sm text-gray-200">{beliefState.current}</p>
              <div className="mt-2 h-1.5 bg-gray-700 rounded-full overflow-hidden">
                <div
                  className="h-full bg-emerald-500"
                  style={{ width: `${beliefState.confidence * 100}%` }}
                />
              </div>
              <span className="text-[10px] text-gray-500">
                {(beliefState.confidence * 100).toFixed(0)}% confidence
              </span>
            </div>
            <div className="flex flex-col gap-2">
              {beliefState.timeline.map((step) => (
                <div key={step.label} className="flex items-center gap-2">
                  <span
                    className={`w-2.5 h-2.5 rounded-full ${
                      step.state === "done"
                        ? "bg-emerald-400"
                        : step.state === "current"
                        ? "bg-violet-400"
                        : "bg-gray-600"
                    }`}
                  />
                  <span
                    className={`text-[11px] ${
                      step.state === "current" ? "text-violet-300" : "text-gray-400"
                    }`}
                  >
                    {step.label}
                  </span>
                </div>
              ))}
            </div>
            <div className="mt-2 bg-violet-500/10 rounded px-3 py-2">
              <p className="text-[10px] uppercase tracking-widest text-violet-300">Next action</p>
              <p className="text-xs text-gray-200">{beliefState.nextAction}</p>
            </div>
          </div>
        </CardShell>
      </div>
    </div>
  );
}
