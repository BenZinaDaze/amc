import { invoke } from "@tauri-apps/api/core";

export type Workspace = { path: string };
export type Agent = { installed: boolean; version: string };
export type McpServer = { name: string; config: unknown; source: string; enabled: boolean; managed: boolean };
export type Skill = { name: string; path: string; source: string; managed: boolean; shadowed: boolean; description: string };
export type Repository = { id: number; url: string; reference: string; localPath?: string };
export type Installation = { id: number; repositoryId: number; skillPath: string; name: string; targetPath: string; commit: string; modified: boolean; updateAvailable: boolean; active: boolean; rollbackAvailable: boolean };
export type State = { workspace: Workspace; agent: Agent; mcp: McpServer[]; skills: Skill[]; repositories: Repository[]; installations: Installation[] };
export type RepositorySkill = { path: string; name: string; description: string };
export type Plan = { id: string; summary: string; changes: { path: string; before: string; after: string }[]; warnings: string[] };
export type Message = { message: string };
export type UsageRange = "1h" | "24h" | "7d" | "14d" | "30d" | "90d" | "all" | `custom:${number}:${number}`;
export type UsageStats = {
  totalRequests: number;
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheRate: number;
  totalCost: number | null;
  unpricedRequests: number;
  byModel: { provider: string; model: string; requests: number; totalTokens: number; cacheRate: number; cost: number | null; unpricedRequests: number }[];
  trend: { bucket: number; requests: number; totalTokens: number }[];
  syncedAt: number;
};
export type QuotaDetail = { name: string; usage: number };
export type QuotaUsage = {
  kind: string;
  label: string;
  usedPercent: number;
  total: number | null;
  used: number | null;
  remaining: number | null;
  resetsAt: number | null;
  windowMinutes: number | null;
  details: QuotaDetail[];
};
export type MetricUsage = { id: string; label: string; value: number };
export type SubscriptionStatus = {
  id: string;
  provider: string;
  title: string;
  platform: string;
  keyHint: string | null;
  error: string | null;
  plan: string | null;
  quotas: QuotaUsage[];
  metrics: MetricUsage[];
};
export type SubscriptionKind = { id: string; title: string; keyLabel: string; keyPlaceholder: string; platforms: [string, string][] };
export type ClaudeCodeStatus = { installed: boolean; version: string };
export type CodexStatus = { installed: boolean; version: string };
export const api = {
  getDefaultWorkspace: () => invoke<Workspace>("get_default_workspace"),
  getState: (workspace: Workspace) => invoke<State>("get_state", { workspace }),
  syncAgentUsage: (agentId: string, range: UsageRange) => invoke<UsageStats>("sync_agent_usage", { agentId, range }),
  getAgentUsage: (agentId: string, range: UsageRange) => invoke<UsageStats>("get_agent_usage", { agentId, range }),
  syncAgentsUsage: (range: UsageRange) => invoke<UsageStats>("sync_agents_usage", { range }),
  getAgentsUsage: (range: UsageRange) => invoke<UsageStats>("get_agents_usage", { range }),
  getClaudeCodeStatus: () => invoke<ClaudeCodeStatus>("get_claude_code_status"),
  getCodexStatus: () => invoke<CodexStatus>("get_codex_status"),
  refreshPricing: () => invoke<string>("refresh_pricing"),
  planMcp: (workspace: Workspace, name: string, config: Record<string, unknown> | null) => invoke<Plan>("plan_mcp", { workspace, name, config }),
  applyPlan: (id: string) => invoke<Message>("apply_plan", { id }),
  addRepository: (url: string, reference: string) => invoke<Repository>("add_repository", { url, reference }),
  removeRepository: (repositoryId: number) => invoke<Message>("remove_repository", { repositoryId }),
  listRepositorySkills: (repositoryId: number) => invoke<RepositorySkill[]>("list_repository_skills", { repositoryId }),
  checkUpdates: (repositoryId: number) => invoke<Message>("check_updates", { repositoryId }),
  checkAllUpdates: () => invoke<Message>("check_all_updates"),
  planSkill: (workspace: Workspace, repositoryId: number, skillPath: string) => invoke<Plan>("plan_skill", { workspace, repositoryId, skillPath }),
  planSync: (installationId: number) => invoke<Plan>("plan_sync", { installationId }),
  planRemoveSkill: (installationId: number) => invoke<Plan>("plan_remove_skill", { installationId }),
  rollbackSkill: (installationId: number) => invoke<Plan>("rollback_skill", { installationId }),
  fetchSubscriptions: () => invoke<SubscriptionStatus[]>("fetch_subscriptions"),
  listSubscriptionKinds: () => invoke<SubscriptionKind[]>("list_subscription_kinds"),
  addSubscriptionPlan: (kind: string, name: string, platform: string, key: string) => invoke<Message>("add_subscription_plan", { kind, name, platform, key }),
  updateSubscriptionPlan: (id: string, name: string, platform: string, key: string) => invoke<Message>("update_subscription_plan", { id, name, platform, key: key || null }),
  removeSubscriptionPlan: (id: string) => invoke<Message>("remove_subscription_plan", { id }),
};
