import type { AgentId } from "./api";

/** Display names shared by every screen, so no ternary drifts. */
export const AGENT_LABELS: Record<AgentId, string> = {
  codex: "Codex",
  claude: "Claude Code",
  omp: "Oh My Pi",
};

export const AGENT_ORDER: AgentId[] = ["codex", "claude", "omp"];

export function agentLabel(agent: AgentId): string {
  return AGENT_LABELS[agent] ?? agent;
}
