import { fetchWithL402 } from "./client";

export async function createSession(budgetSats: number) {
  return fetchWithL402("/v1/session/new", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ budget_sats: budgetSats }),
  });
}
