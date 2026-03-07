/**
 * REST API client for skill health endpoints.
 */

import { ensureConnection } from "./connection";
import type { SkillHealthMetrics } from "../types";

export async function getSkillHealth(): Promise<SkillHealthMetrics[]> {
  const conn = await ensureConnection();
  const res = await fetch(`${conn.baseUrl}/v1/skills/health`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });
  if (!res.ok) throw new Error(`Failed to fetch skill health: ${res.statusText}`);
  return await res.json();
}
