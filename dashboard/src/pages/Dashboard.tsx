import { motion } from "framer-motion";
import type { TimeRange } from "../hooks/useStats";
import { StatsBar } from "../components/StatsBar";
import { SpendOverTime } from "../components/charts/SpendOverTime";
import { RequestsOverTime } from "../components/charts/RequestsOverTime";
import { CostDistribution } from "../components/charts/CostDistribution";
import { WasteScore } from "../components/charts/WasteScore";
import { TopTracesTable } from "../components/TopTracesTable";
import { LiveIndicator } from "../components/common/LiveIndicator";

interface Props {
  timeRange: TimeRange;
}

const stagger = {
  hidden: {},
  show: {
    transition: { staggerChildren: 0.08 },
  },
};

const fadeUp = {
  hidden: { opacity: 0, y: 16 },
  show: {
    opacity: 1,
    y: 0,
    transition: { duration: 0.5, ease: [0.16, 1, 0.3, 1] },
  },
};

export function Dashboard({ timeRange }: Props) {
  return (
    <motion.div
      variants={stagger}
      initial="hidden"
      animate="show"
      className="flex flex-col gap-6 max-w-[1600px] mx-auto"
    >
      {/* Header */}
      <motion.div variants={fadeUp} className="flex items-center gap-3">
        <h1 className="text-[0.625rem] font-semibold tracking-[0.15em] uppercase text-[var(--text-secondary)]">
          Mission Control
        </h1>
        <LiveIndicator />
      </motion.div>

      {/* Row 1: summary stat cards */}
      <motion.div variants={fadeUp}>
        <StatsBar timeRange={timeRange} />
      </motion.div>

      {/* Row 2: time-series charts side by side */}
      <motion.div variants={fadeUp} className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <SpendOverTime timeRange={timeRange} />
        <RequestsOverTime timeRange={timeRange} />
      </motion.div>

      {/* Row 3: cost breakdown + waste score */}
      <motion.div variants={fadeUp} className="grid grid-cols-1 lg:grid-cols-3 gap-4">
        <div className="lg:col-span-2">
          <CostDistribution timeRange={timeRange} />
        </div>
        <WasteScore timeRange={timeRange} />
      </motion.div>

      {/* Row 4: top traces table */}
      <motion.div variants={fadeUp}>
        <TopTracesTable timeRange={timeRange} />
      </motion.div>
    </motion.div>
  );
}
