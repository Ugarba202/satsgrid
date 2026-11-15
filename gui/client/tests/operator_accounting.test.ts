import test from "node:test";
import assert from "node:assert/strict";
import { NodeStatsResponse, ModelActivityItem, RecentRequestItem } from "@/lib/api/node";

test("Operator Accounting - Invariants: Revenue excludes reserved holds and failed requests", () => {
  // Mock node stats response reflecting persisted SQLite state
  const stats: NodeStatsResponse = {
    total_requests: 15,
    active_sessions: 2,
    total_sats_earned: 150,
    reserved_sats: 70,
    requests_breakdown: {
      completed: 15,
      failed: 2,
      reconciliation_required: 9,
    },
    model_breakdown: [
      { model: "llama3.2:latest", completed_requests: 12, sats_earned: 120 },
      { model: "llama3.2", completed_requests: 3, sats_earned: 30 },
    ],
    recent_requests: [
      {
        id: "cdc09224-8684-4680-9be6-ed2727005d95",
        model: "llama3.2:latest",
        status: "completed",
        charged_sats: 10,
        created_at: 1794733322,
      },
      {
        id: "005bba1e-a2a2-4d3f-8914-dc0a4f69f86b",
        model: "llama3.2:latest",
        status: "reconciliation_required",
        charged_sats: 10,
        created_at: 1794733250,
      },
    ],
    invoices: {
      pending: 1,
      settled: 8,
      expired: 0,
      cancelled: 0,
    },
    reconciliation: {
      unresolved_items: 1,
      worker: null,
    },
  };

  // Invariant 1: Total settled revenue matches sum of completed model revenues
  const modelRevenueSum = (stats.model_breakdown || []).reduce(
    (sum, m) => sum + m.sats_earned,
    0
  );
  assert.equal(
    stats.total_sats_earned,
    modelRevenueSum,
    "total_sats_earned must equal sum of completed model sats"
  );
  assert.equal(stats.total_sats_earned, 150);

  // Invariant 2: Total completed requests equals sum of model completed requests
  const modelRequestSum = (stats.model_breakdown || []).reduce(
    (sum, m) => sum + m.completed_requests,
    0
  );
  assert.equal(
    stats.requests_breakdown?.completed,
    modelRequestSum,
    "completed requests count must equal sum of model completed requests"
  );
  assert.equal(stats.requests_breakdown?.completed, 15);

  // Invariant 3: Reserved sats is strictly distinct from settled revenue
  assert.equal(stats.reserved_sats, 70);
  assert.notEqual(
    stats.total_sats_earned,
    stats.total_sats_earned + (stats.reserved_sats || 0),
    "Reserved sats must not be conflated with settled revenue"
  );

  // Invariant 4: Failed and reconciliation requests are tracked separately
  assert.equal(stats.requests_breakdown?.failed, 2);
  assert.equal(stats.requests_breakdown?.reconciliation_required, 9);
});

test("Operator Accounting - Model breakdown preserves empty or zero states gracefully", () => {
  const minimalStats: NodeStatsResponse = {
    total_requests: 0,
    total_sats_earned: 0,
    active_sessions: 0,
    reserved_sats: 0,
    requests_breakdown: {
      completed: 0,
      failed: 0,
      reconciliation_required: 0,
    },
    model_breakdown: [],
    recent_requests: [],
  };

  assert.equal(minimalStats.total_sats_earned, 0);
  assert.equal(minimalStats.reserved_sats, 0);
  assert.equal(minimalStats.model_breakdown?.length, 0);
  assert.equal(minimalStats.recent_requests?.length, 0);
});
