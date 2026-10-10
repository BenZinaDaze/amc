import { invoke } from "@tauri-apps/api/core";

export type Workspace = { path: string };
export type Agent = { installed: boolean; version: string };
export type McpAgent = "omp" | "claude" | "codex";
export type McpServer = { name: string; config: Record<string, unknown>; agents: McpAgent[] };
export type SkillTarget = "omp" | "codex" | "claude";
export type Skill = { id: number; name: string; description: string; repositoryId: number; skillPath: string; commit: string; updateAvailable: boolean; omp: boolean; codex: boolean; claude: boolean };
export type DetectedSkill = { name: string; path: string; source: string; shadowed: boolean; description: string };
export type Repository = { id: number; url: string; reference: string; localPath?: string };
export type State = { workspace: Workspace; agent: Agent; installedAgents: McpAgent[]; mcp: McpServer[]; skills: Skill[]; detected: DetectedSkill[]; repositories: Repository[] };
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
  byModel: { model: string; requests: number; totalTokens: number; cacheRate: number; cost: number | null; unpricedRequests: number }[];
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
  unit: "usd" | "cny" | null;
};
export type MetricUsage = { id: string; label: string; value: number };
export type SubscriptionStatus = {
  id: string;
  provider: string;
  title: string;
  platform: string;
  baseUrl: string | null;
  keyHint: string | null;
  error: string | null;
  plan: string | null;
  quotas: QuotaUsage[];
  metrics: MetricUsage[];
  pending: boolean;
};
export type SubscriptionKind = { id: string; title: string; keyLabel: string; keyPlaceholder: string; auth: "key" | "oauth"; platforms: [string, string][]; urlLabel: string | null; urlPlaceholder: string | null };
export type ClaudeCodeStatus = { installed: boolean; version: string };
export type CodexStatus = { installed: boolean; version: string };
export type AgentPathKind = "file" | "dir";
export type ConfigPathEntry = { purpose: string; path: string; kind: AgentPathKind; exists: boolean };
export type AgentConfigPaths = { agent: McpAgent; entries: ConfigPathEntry[] };
export type AppUpdate = { latest: string; url: string };
/// 已接入 Agent 的更新来源 id（对应后端 check_agent_update / update_agent）。
export type AgentUpdateSource = "omp" | "claude-code" | "codex";
export const api = {
  getDefaultWorkspace: () => invoke<Workspace>("get_default_workspace"),
  getState: (workspace: Workspace) => invoke<State>("get_state", { workspace }),
  syncAgentUsage: (agentId: string, range: UsageRange) => invoke<UsageStats>("sync_agent_usage", { agentId, range }),
  getAgentUsage: (agentId: string, range: UsageRange) => invoke<UsageStats>("get_agent_usage", { agentId, range }),
  syncAgentsUsage: (range: UsageRange) => invoke<UsageStats>("sync_agents_usage", { range }),
  getAgentsUsage: (range: UsageRange) => invoke<UsageStats>("get_agents_usage", { range }),
  getClaudeCodeStatus: () => invoke<ClaudeCodeStatus>("get_claude_code_status"),
  getCodexStatus: () => invoke<CodexStatus>("get_codex_status"),
  getAgentConfigPaths: () => invoke<AgentConfigPaths[]>("get_agent_config_paths"),
  refreshPricing: () => invoke<string>("refresh_pricing"),
  planMcp: (name: string, config: Record<string, unknown> | null, agents: McpAgent[]) => invoke<Plan>("plan_mcp", { name, config, agents }),
  planMcpToggle: (name: string, agent: McpAgent, enabled: boolean) => invoke<Plan>("plan_mcp_toggle", { name, agent, enabled }),
  applyPlan: (id: string) => invoke<Message>("apply_plan", { id }),
  addRepository: (url: string, reference: string) => invoke<Repository>("add_repository", { url, reference }),
  removeRepository: (repositoryId: number) => invoke<Message>("remove_repository", { repositoryId }),
  listRepositorySkills: (repositoryId: number) => invoke<RepositorySkill[]>("list_repository_skills", { repositoryId }),
  checkUpdates: (repositoryId: number) => invoke<Message>("check_updates", { repositoryId }),
  checkAllUpdates: () => invoke<Message>("check_all_updates"),
  planSkill: (repositoryId: number, skillPath: string) => invoke<Plan>("plan_skill", { repositoryId, skillPath }),
  planSkillToggle: (name: string, target: SkillTarget, enabled: boolean) => invoke<Plan>("plan_skill_toggle", { name, target, enabled }),
  planSkillRemove: (name: string) => invoke<Plan>("plan_skill_remove", { name }),
  fetchSubscriptions: (nonce: number) => invoke<SubscriptionStatus[]>("fetch_subscriptions", { nonce }),
  listSubscriptionKinds: () => invoke<SubscriptionKind[]>("list_subscription_kinds"),
  addSubscriptionPlan: (kind: string, name: string, platform: string, key: string, baseUrl: string) => invoke<Message>("add_subscription_plan", { kind, name, platform, key, baseUrl: baseUrl || null }),
  updateSubscriptionPlan: (id: string, name: string, platform: string, key: string, baseUrl: string) => invoke<Message>("update_subscription_plan", { id, name, platform, key: key || null, baseUrl: baseUrl || null }),
  antigravityLogin: (name: string) => invoke<Message>("antigravity_login_and_add", { name }),
  cursorLogin: (name: string) => invoke<Message>("cursor_login_and_add", { name }),
  removeSubscriptionPlan: (id: string) => invoke<Message>("remove_subscription_plan", { id }),
  checkAppUpdate: () => invoke<AppUpdate>("check_app_update"),
  checkAgentUpdate: (agent: AgentUpdateSource) => invoke<AppUpdate>("check_agent_update", { agent }),
  updateAgent: (agent: AgentUpdateSource) => invoke<string>("update_agent", { agent }),
};
