import { useEffect, useState, type FormEvent } from "react";
import { Glyph } from "../../components/Glyph";
import { AgentMark, agentMeta } from "../../components/AgentMark";
import type { McpAgent, McpServer } from "../../api";
import { errorText } from "../../utils";

type McpMode = "stdio" | "http" | "sse";

function parseConfigObject(text: string): Record<string, unknown> {
  const value: unknown = JSON.parse(text);
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("配置必须是 JSON 对象");
  return value as Record<string, unknown>;
}

/** 兼容从其它工具文档复制的整块 JSON：识别 mcpServers 包装并解包。 */
function unwrapMcpEnvelope(config: Record<string, unknown>, fallbackName: string): { name: string; spec: Record<string, unknown> } {
  if (!("mcpServers" in config)) return { name: fallbackName, spec: config };
  const servers = config.mcpServers;
  if (!servers || typeof servers !== "object" || Array.isArray(servers)) throw new Error("mcpServers 必须是 JSON 对象");
  const entries = Object.entries(servers as Record<string, unknown>);
  if (entries.length !== 1) throw new Error("每次只支持一个 MCP 服务；请保留 mcpServers 中的一项，或直接粘贴该服务的配置对象。");
  const [key, spec] = entries[0];
  if (!spec || typeof spec !== "object" || Array.isArray(spec)) throw new Error("MCP 服务配置必须是 JSON 对象");
  return { name: key, spec: spec as Record<string, unknown> };
}

/// 添加 / 编辑 MCP 服务弹窗：结构化编辑与整块 JSON 双模式，
/// 保存生成写入预览（不在弹窗内直接写盘）。
export function McpFormModal({ server, existingNames, installedAgents, isBusy, error, setError, onClose, onPreview }: {
  server?: McpServer;
  existingNames: string[];
  installedAgents: McpAgent[];
  isBusy: boolean;
  error: string;
  setError: (message: string) => void;
  onClose: () => void;
  onPreview: (name: string, config: Record<string, unknown>, agents: McpAgent[]) => Promise<boolean>;
}) {
  const original = server?.name || "";
  const [name, setName] = useState(original);
  const [mode, setMode] = useState<McpMode>(() => {
    const config = server?.config;
    return config?.type === "sse" ? "sse" : config?.type === "http" || typeof config?.url === "string" ? "http" : "stdio";
  });
  const [command, setCommand] = useState(() => typeof server?.config.command === "string" ? server.config.command : "");
  const [args, setArgs] = useState(() => Array.isArray(server?.config.args) ? server.config.args.map(String).join("\n") : "");
  const [url, setUrl] = useState(() => typeof server?.config.url === "string" ? server.config.url : "");
  const [extra, setExtra] = useState(() => {
    const config = server?.config ?? {};
    const extras = { ...config }; delete extras.type; delete extras.command; delete extras.args; delete extras.url; delete extras.enabled;
    return JSON.stringify(extras, null, 2);
  });
  const [raw, setRaw] = useState(() => JSON.stringify(server?.config ?? {}, null, 2));
  const [agentFlags, setAgentFlags] = useState<Record<McpAgent, boolean>>(() => ({ omp: false, claude: false, codex: false, ...(server ? Object.fromEntries(server.agents.map((agent) => [agent, true])) : {}) }));
  const [rawConfig, setRawConfig] = useState(() => Boolean(server?.config.type && !["stdio", "http", "sse"].includes(String(server.config.type))));

  // Escape 关闭与背景点击一致：仅在非忙碌时生效。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && !isBusy) onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isBusy, onClose]);

  function structuredConfig(requireConnection: boolean): Record<string, unknown> {
    const extras = parseConfigObject(extra);
    if (mode === "stdio") {
      if (requireConnection && !command.trim()) throw new Error("请输入可执行命令");
      return { ...extras, type: "stdio", command: command.trim(), args: args.split("\n").filter((arg) => arg.length > 0) };
    }
    if (requireConnection && !url.trim()) throw new Error("请输入服务 URL");
    return { ...extras, type: mode, url: url.trim() };
  }

  function switchEditor() {
    try {
      if (!rawConfig) {
        setRaw(JSON.stringify(structuredConfig(false), null, 2));
      } else {
        const config = parseConfigObject(raw);
        if (config.type && !["stdio", "http", "sse"].includes(String(config.type))) throw new Error("此连接类型只能在完整 JSON 模式下编辑");
        const nextMode: McpMode = config.type === "sse" ? "sse" : config.type === "http" || typeof config.url === "string" ? "http" : "stdio";
        const extras = { ...config }; delete extras.type; delete extras.command; delete extras.args; delete extras.url; delete extras.enabled;
        setMode(nextMode);
        setCommand(typeof config.command === "string" ? config.command : "");
        setArgs(Array.isArray(config.args) ? config.args.map(String).join("\n") : "");
        setUrl(typeof config.url === "string" ? config.url : "");
        setExtra(JSON.stringify(extras, null, 2));
      }
      setError("");
      setRawConfig(!rawConfig);
    } catch (reason) { setError(`无法切换编辑模式：${errorText(reason)}`); }
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!name.trim()) { setError("请输入 MCP 服务名称。"); return; }
    let resolvedName = name.trim();
    let config: Record<string, unknown>;
    try {
      const parsed = rawConfig ? parseConfigObject(raw) : structuredConfig(true);
      ({ name: resolvedName, spec: config } = unwrapMcpEnvelope(parsed, resolvedName));
    }
    catch (reason) { setError(`配置无效：${errorText(reason)}`); return; }
    if (original && original !== resolvedName) { setError("修改服务名称需先移除旧服务，再添加新服务。"); return; }
    if (existingNames.some((existing) => existing === resolvedName && existing !== original)) { setError("同名 MCP 服务已存在。"); return; }
    const agents = (Object.keys(agentFlags) as McpAgent[]).filter((agent) => agentFlags[agent]);
    const ok = await onPreview(resolvedName, config, agents);
    if (ok) onClose();
  }

  return <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !isBusy) onClose(); }}>
    <div className="modal" role="dialog" aria-modal="true" aria-labelledby="mcp-title">
      <div className="modal-head">
        <div><div className="eyebrow">MCP / 配置</div><h2 id="mcp-title">{original ? "编辑服务" : "添加 MCP 服务"}</h2></div>
        <button className="icon-button" aria-label="关闭" disabled={isBusy} onClick={onClose}><Glyph name="close" size={20} /></button>
      </div>
      <form onSubmit={(event) => void save(event)} className="modal-body mcp-form">
        {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button type="button" aria-label="关闭错误提示" onClick={() => setError("")}><Glyph name="close" size={16} /></button></div>}
        <label>服务名称<input value={name} onChange={(event) => setName(event.target.value)} placeholder="例如 filesystem" required readOnly={Boolean(original)} /></label>
        {!rawConfig && <div>
          <span className="field-label">写入到哪些 Agent</span>
          <div className="mcp-agents">
            {(["omp", "claude", "codex"] as McpAgent[]).filter((agent) => installedAgents.includes(agent) || agentFlags[agent]).map((agent) => (
              <label key={agent} className="checkbox-row mcp-agent-option" title={installedAgents.includes(agent) ? undefined : "未检测到该 Agent 的安装"}>
                <input type="checkbox" checked={agentFlags[agent]} onChange={(event) => setAgentFlags((flags) => ({ ...flags, [agent]: event.target.checked }))} />
                <AgentMark agent={agent} active={agentFlags[agent] && installedAgents.includes(agent)} />
                {agentMeta[agent].label}{installedAgents.includes(agent) ? "" : "（未检测到安装）"}
              </label>
            ))}
          </div>
          <small>勾选后保存会写入对应 Agent 的用户级配置；之后也可在列表中点击开关切换。</small>
        </div>}
        <label className="checkbox-row"><input type="checkbox" checked={rawConfig} onChange={switchEditor} />直接编辑完整 JSON 配置</label>
        {rawConfig ? <label>配置对象<textarea className="code-editor" spellCheck={false} rows={12} value={raw} onChange={(event) => setRaw(event.target.value)} /></label> : <>
          <div>
            <span className="field-label">连接方式</span>
            <div className="segmented">
              <button type="button" className={mode === "stdio" ? "active" : ""} onClick={() => setMode("stdio")}>stdio</button>
              <button type="button" className={mode === "http" ? "active" : ""} onClick={() => setMode("http")}>HTTP</button>
              <button type="button" className={mode === "sse" ? "active" : ""} onClick={() => setMode("sse")}>SSE</button>
            </div>
          </div>
          {mode === "stdio" ? <>
            <label>命令<input value={command} onChange={(event) => setCommand(event.target.value)} placeholder="例如 npx" required /></label>
            <label>参数（每行一项）<textarea rows={3} value={args} onChange={(event) => setArgs(event.target.value)} placeholder={"-y\n@modelcontextprotocol/server-filesystem"} /></label>
          </> : <label>服务 URL<input type="url" value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://example.com/mcp" required /></label>}
          <label>附加配置（JSON 对象，可选）<textarea className="code-editor" spellCheck={false} rows={5} value={extra} onChange={(event) => setExtra(event.target.value)} placeholder='{"env": {"KEY": "value"}}' /><small>可填写 env、headers 等字段；不会执行服务命令。</small></label>
        </>}
        <div className="modal-actions">
          <button type="button" className="button button-muted" disabled={isBusy} onClick={onClose}>取消</button>
          <button type="submit" className="button button-primary" disabled={isBusy}>{isBusy ? "正在生成…" : "生成预览"}</button>
        </div>
      </form>
    </div>
  </div>;
}
