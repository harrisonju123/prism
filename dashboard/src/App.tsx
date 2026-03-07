import { useState } from "react";
import { BrowserRouter, Routes, Route, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AnimatePresence } from "framer-motion";
import { Shell } from "./components/layout/Shell";
import { Dashboard } from "./pages/Dashboard";
import { Events } from "./pages/Events";
import { Alerts } from "./pages/Alerts";
import { Benchmarks } from "./pages/Benchmarks";
import { WasteDetails } from "./pages/WasteDetails";
import { Routing } from "./pages/Routing";
import { MCPTracing } from "./pages/MCPTracing";
import { PageTransition } from "./components/common/PageTransition";
import type { TimeRange } from "./hooks/useStats";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        if (error instanceof Error && error.message.includes("API error: 4")) {
          return false;
        }
        return failureCount < 2;
      },
    },
  },
});

function AnimatedRoutes({ timeRange }: { timeRange: TimeRange }) {
  const location = useLocation();

  return (
    <AnimatePresence mode="wait">
      <PageTransition key={location.pathname}>
        <Routes location={location}>
          <Route path="/" element={<Dashboard timeRange={timeRange} />} />
          <Route path="/events" element={<Events timeRange={timeRange} />} />
          <Route path="/alerts" element={<Alerts />} />
          <Route path="/benchmarks" element={<Benchmarks timeRange={timeRange} />} />
          <Route path="/waste" element={<WasteDetails timeRange={timeRange} />} />
          <Route path="/routing" element={<Routing />} />
          <Route path="/mcp" element={<MCPTracing timeRange={timeRange} />} />
        </Routes>
      </PageTransition>
    </AnimatePresence>
  );
}

export default function App() {
  const [timeRange, setTimeRange] = useState<TimeRange>("30d");

  return (
    <BrowserRouter basename="/dashboard">
      <QueryClientProvider client={queryClient}>
        <Shell timeRange={timeRange} onTimeRangeChange={setTimeRange}>
          <AnimatedRoutes timeRange={timeRange} />
        </Shell>
      </QueryClientProvider>
    </BrowserRouter>
  );
}
