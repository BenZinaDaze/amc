import { useCallback, useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Glyph } from "../../components/Glyph";
import { AgentMark, agentMeta } from "../../components/AgentMark";
import { api, type AgentConfigPaths } from "../../api";
import { errorText } from "../../utils";

/// 配置文件 tab：按 Agent 分组列出用户级配置文件/目录的实际位置。
/// 路径由后端按各适配器同一套定位规则解析（环境变量与 profile 一并生效）。
export function ConfigPathsPanel() {
  const [groups, setGroups] = useState<AgentConfigPaths[] | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  const load = useCallback(async (initial: boolean) => {
    if (initial) setGroups(null);
    setLoading(true);
    setError("");
    try {
      setGroups(await api.getAgentConfigPaths());
    } catch (reason) {
      setError(`无法解析配置路径：${errorText(reason)}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void load(true); }, [load]);

  async function reveal(path: string) {
    try {
      await revealItemInDir(path);
    } catch (reason) {
      setError(`无法在文件管理器中显示：${errorText(reason)}`);
    }
  }

  return <>
    {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button aria-label="关闭错误提示" onClick={() => setError("")}><Glyph name="close" size={16} /></button></div>}
    <div className="settings-toolbar">
      <p className="settings-hint">以下为各 Agent 生效的用户级配置；AMC 仅写这些位置，且都在预览确认后写入。</p>
      <button className="button button-muted" disabled={loading} onClick={() => void load(false)}><Glyph name="refresh" size={16} />刷新</button>
    </div>
    {!groups ? <div className="loading-panel" role="status"><span className="spinner" />正在解析配置路径…</div> : groups.map((group) => <section className="path-card" key={group.agent}>
      <div className="path-card-head">
        <AgentMark agent={group.agent} active />
        <h3>{agentMeta[group.agent].label}</h3>
        <span className="path-count">{group.entries.length} 项</span>
      </div>
      {group.entries.map((entry) => <div className="path-row" key={entry.path}>
        <span className="path-kind" title={entry.kind === "dir" ? "目录" : "文件"}><Glyph name={entry.kind === "dir" ? "folder" : "file"} size={15} /></span>
        <span className="path-purpose">{entry.purpose}</span>
        <span className="path-value" title={entry.path}>{entry.path}</span>
        <span className={`tag ${entry.exists ? "tag-good" : "tag-muted"}`}>{entry.exists ? "存在" : "未创建"}</span>
        <button type="button" className="icon-button" disabled={!entry.exists} title={entry.exists ? "在文件管理器中显示" : "路径尚未创建"} aria-label={`在文件管理器中显示 ${entry.path}`} onClick={() => void reveal(entry.path)}><Glyph name="external" size={15} /></button>
      </div>)}
    </section>)}
  </>;
}
