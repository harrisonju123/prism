import { useMemo, useState } from "react";
import { CardShell } from "../components/common/CardShell";
import { InfoTip } from "../components/common/InfoTip";
import {
  useCreateDebugExperiment,
  useCreateDebugHypothesis,
  useCreateDebugRun,
  useCreateDebugSession,
  useDebugSession,
  useDebugSessions,
} from "../hooks/useDebug";
import type {
  DebugExperiment,
  DebugRun,
  DebugSessionSummary,
} from "../api/types";

const LOOP_STEPS = [
  {
    id: "symptom",
    title: "Symptom",
    description:
      "Capture the failing replay/test, impact radius, and raw output.",
  },
  {
    id: "hypotheses",
    title: "Hypotheses",
    description: "Ranked list with confidence + evidence.",
  },
  {
    id: "experiments",
    title: "Proposed Experiments",
    description: "Label cost/impact before running.",
  },
  {
    id: "run",
    title: "Run Experiment",
    description: "Execute the experiment and capture output.",
  },
  {
    id: "update",
    title: "Update Belief",
    description: "Update confidence + decide next action.",
  },
];

function levelPill(level: string) {
  const base = "text-[10px] px-1.5 py-0.5 rounded border";
  if (level === "low")
    return `${base} bg-emerald-500/10 text-emerald-300 border-emerald-500/20`;
  if (level === "high")
    return `${base} bg-red-500/10 text-red-300 border-red-500/20`;
  return `${base} bg-amber-500/10 text-amber-300 border-amber-500/20`;
}

function formatRunOutput(run?: DebugRun) {
  if (!run?.output) return "No output captured.";
  return run.output;
}

function asRecord(value: unknown): Record<string, unknown> {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return {};
}

function readString(value: unknown, fallback: string) {
  return typeof value === "string" ? value : fallback;
}

export function Debugging() {
  const { data: sessions, isLoading: sessionsLoading } = useDebugSessions();
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(
    null,
  );
  const { data: sessionDetail, isLoading: sessionLoading } = useDebugSession(
    selectedSessionId ?? undefined,
  );

  const createSession = useCreateDebugSession();
  const createHypothesis = useCreateDebugHypothesis(selectedSessionId ?? "");
  const createExperiment = useCreateDebugExperiment(selectedSessionId ?? "");

  const latestExperiment = useMemo<DebugExperiment | null>(() => {
    if (!sessionDetail?.experiments.length) return null;
    return sessionDetail.experiments[sessionDetail.experiments.length - 1];
  }, [sessionDetail?.experiments]);

  const createRun = useCreateDebugRun(
    selectedSessionId ?? "",
    latestExperiment?.id ?? "",
  );

  const latestRun = useMemo<DebugRun | null>(() => {
    if (!sessionDetail?.runs.length) return null;
    return sessionDetail.runs[sessionDetail.runs.length - 1];
  }, [sessionDetail?.runs]);

  const hasSessions = (sessions?.length ?? 0) > 0;
  const symptom = asRecord(sessionDetail?.session.symptom);
  const metadata = asRecord(sessionDetail?.session.metadata);

  return (
    <div className="flex flex-col gap-6 max-w-[1600px] mx-auto">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <h1 className="text-lg font-semibold">IDE Debugging</h1>
          <InfoTip text="Phase 1: debug sessions are persisted and visible here. Agent automation comes next." />
        </div>
        <button
          className="px-3 py-1.5 text-xs bg-violet-600/20 text-violet-200 rounded hover:bg-violet-600/30 transition-colors"
          onClick={() => {
            const title = window.prompt("Session title");
            if (!title?.trim()) return;
            createSession.mutate({ title: title.trim() });
          }}
          disabled={createSession.isPending}
        >
          New session
        </button>
      </div>

      <CardShell title="Hypothesis Loop">
        <div className="grid grid-cols-1 md:grid-cols-5 gap-3">
          {LOOP_STEPS.map((step, index) => (
            <div
              key={step.id}
              className="flex flex-col gap-2 bg-white/[0.02] border border-white/[0.06] rounded-md p-3"
            >
              <div className="flex items-center gap-2">
                <span className="text-[10px] px-2 py-0.5 rounded-full bg-violet-500/15 text-violet-300 font-semibold">
                  {index + 1}
                </span>
                <span className="text-xs font-semibold text-gray-200">
                  {step.title}
                </span>
              </div>
              <p className="text-[11px] text-gray-400 leading-relaxed">
                {step.description}
              </p>
            </div>
          ))}
        </div>
      </CardShell>

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <CardShell title="Sessions">
          {sessionsLoading && (
            <p className="text-xs text-gray-500">Loading sessions…</p>
          )}
          {!sessionsLoading && !hasSessions && (
            <p className="text-xs text-gray-500">No debug sessions yet.</p>
          )}
          {hasSessions && (
            <div className="flex flex-col gap-2">
              {sessions?.map((session: DebugSessionSummary) => (
                <button
                  key={session.id}
                  onClick={() => setSelectedSessionId(session.id)}
                  className={`text-left px-3 py-2 rounded border transition-colors ${
                    selectedSessionId === session.id
                      ? "border-violet-500/40 bg-violet-500/10 text-violet-200"
                      : "border-white/[0.06] bg-white/[0.02] text-gray-300 hover:bg-white/[0.04]"
                  }`}
                >
                  <div className="text-xs font-medium">{session.title}</div>
                  <div className="text-[10px] text-gray-500">
                    {new Date(session.created_at).toLocaleString("en-US", {
                      month: "short",
                      day: "numeric",
                      hour: "numeric",
                      minute: "2-digit",
                    })}
                  </div>
                </button>
              ))}
            </div>
          )}
        </CardShell>

        <CardShell title="Active Symptom" className="lg:col-span-2">
          {sessionLoading && (
            <p className="text-xs text-gray-500">Loading session…</p>
          )}
          {!sessionLoading && !sessionDetail && (
            <p className="text-xs text-gray-500">
              Select a session to view details.
            </p>
          )}
          {sessionDetail && (
            <div className="flex flex-col gap-3 text-xs">
              <div className="bg-red-500/10 border border-red-500/20 rounded-md p-3">
                <div className="text-[10px] uppercase text-red-300 font-semibold">
                  Symptom
                </div>
                <div className="text-sm font-mono text-red-200 mt-1">
                  {readString(symptom.summary, "Symptom captured")}
                </div>
                <div className="text-[10px] text-red-300/70 mt-2">
                  {readString(symptom.source, "Source unknown")}
                </div>
              </div>
              <div className="flex items-center justify-between text-[11px] text-gray-400">
                <span>Status</span>
                <span className="text-gray-200">
                  {sessionDetail.session.status}
                </span>
              </div>
              <div className="flex items-center justify-between text-[11px] text-gray-400">
                <span>Owner</span>
                <span className="text-gray-200">
                  {readString(metadata.owner, "—")}
                </span>
              </div>
            </div>
          )}
        </CardShell>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <CardShell title="Ranked Hypotheses">
          {!sessionDetail && (
            <p className="text-xs text-gray-500">
              Add a session to track hypotheses.
            </p>
          )}
          {sessionDetail && (
            <div className="flex flex-col gap-2">
              {sessionDetail.hypotheses.map((hypothesis) => (
                <div
                  key={hypothesis.id}
                  className="flex items-center gap-3 bg-white/[0.02] border border-white/[0.06] rounded-md px-3 py-2"
                >
                  <div className="flex-1">
                    <div className="text-xs text-gray-200">
                      {hypothesis.statement}
                    </div>
                    <div className="text-[10px] text-gray-500">
                      {hypothesis.status}
                    </div>
                  </div>
                  <div className="text-xs font-mono text-emerald-300">
                    {(hypothesis.confidence * 100).toFixed(0)}%
                  </div>
                </div>
              ))}
              <button
                className="self-start px-3 py-1 text-xs bg-violet-600/20 text-violet-200 rounded hover:bg-violet-600/30 transition-colors"
                disabled={!selectedSessionId || createHypothesis.isPending}
                onClick={() => {
                  const statement = window.prompt("Hypothesis statement");
                  if (!statement?.trim()) return;
                  createHypothesis.mutate({ statement: statement.trim(), confidence: 0.5 });
                }}
              >
                Add hypothesis
              </button>
            </div>
          )}
        </CardShell>

        <CardShell title="Proposed Experiments">
          {!sessionDetail && (
            <p className="text-xs text-gray-500">
              Select a session to plan experiments.
            </p>
          )}
          {sessionDetail && (
            <div className="flex flex-col gap-2">
              {sessionDetail.experiments.map((experiment) => (
                <div
                  key={experiment.id}
                  className="flex items-center justify-between bg-white/[0.02] border border-white/[0.06] rounded-md px-3 py-2"
                >
                  <div>
                    <div className="text-xs text-gray-200">
                      {experiment.title}
                    </div>
                    <div className="text-[10px] text-gray-500">
                      {experiment.status}
                    </div>
                  </div>
                  <div className="flex items-center gap-2">
                    <span className={levelPill(experiment.cost_level)}>
                      cost {experiment.cost_level}
                    </span>
                    <span className={levelPill(experiment.impact_level)}>
                      impact {experiment.impact_level}
                    </span>
                  </div>
                </div>
              ))}
              <button
                className="self-start px-3 py-1 text-xs bg-violet-600/20 text-violet-200 rounded hover:bg-violet-600/30 transition-colors"
                disabled={!selectedSessionId || createExperiment.isPending}
                onClick={() => {
                  const title = window.prompt("Experiment title");
                  if (!title?.trim()) return;
                  createExperiment.mutate({ title: title.trim() });
                }}
              >
                Add experiment
              </button>
            </div>
          )}
        </CardShell>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <CardShell title="Run Experiment">
          {!latestExperiment && (
            <p className="text-xs text-gray-500">
              Create an experiment to run it.
            </p>
          )}
          {latestExperiment && (
            <div className="flex flex-col gap-3 text-xs">
              <div className="flex items-center justify-between text-[11px] text-gray-400">
                <span>Selected</span>
                <span className="text-gray-200">{latestExperiment.title}</span>
              </div>
              <button
                className="self-start px-3 py-1 text-xs bg-emerald-600/20 text-emerald-200 rounded hover:bg-emerald-600/30 transition-colors"
                onClick={() => {
                  const output = window.prompt("Run output (paste stdout/result)");
                  if (output === null) return;
                  createRun.mutate({ status: "completed", output: output || undefined });
                }}
                disabled={createRun.isPending}
              >
                Capture run output
              </button>
              <div className="bg-black/40 border border-white/[0.08] rounded-md p-3 font-mono text-[10px] text-emerald-200/80 whitespace-pre-wrap">
                {formatRunOutput(latestRun ?? undefined)}
              </div>
            </div>
          )}
        </CardShell>

        <CardShell title="Belief State Update">
          {!sessionDetail && (
            <p className="text-xs text-gray-500">
              Belief updates will appear after runs.
            </p>
          )}
          {sessionDetail && (
            <div className="flex flex-col gap-3 text-xs">
              <div className="bg-white/[0.02] border border-white/[0.06] rounded-md p-3">
                <div className="text-[10px] uppercase text-gray-500">
                  Next action
                </div>
                <div className="text-sm text-gray-200 mt-1">
                  Capture the next experiment and update confidence.
                </div>
              </div>
              <div className="flex items-center justify-between text-[11px] text-gray-400">
                <span>Hypotheses</span>
                <span className="text-gray-200">
                  {sessionDetail.hypotheses.length}
                </span>
              </div>
              <div className="flex items-center justify-between text-[11px] text-gray-400">
                <span>Experiments</span>
                <span className="text-gray-200">
                  {sessionDetail.experiments.length}
                </span>
              </div>
              <div className="flex items-center justify-between text-[11px] text-gray-400">
                <span>Runs</span>
                <span className="text-gray-200">
                  {sessionDetail.runs.length}
                </span>
              </div>
            </div>
          )}
        </CardShell>
      </div>
    </div>
  );
}
