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

export const api = {
  getDefaultWorkspace: () => invoke<Workspace>("get_default_workspace"),
  getState: (workspace: Workspace) => invoke<State>("get_state", { workspace }),
  syncAgentUsage: (agentId: string, range: UsageRange) => invoke<UsageStats>("sync_agent_usage", { agentId, range }),
  getAgentUsage: (agentId: string, range: UsageRange) => invoke<UsageStats>("get_agent_usage", { agentId, range }),
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
};
