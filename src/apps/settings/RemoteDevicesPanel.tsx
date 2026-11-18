import { useCallback, useEffect, useRef, useState } from "react";
import { Glyph } from "../../components/Glyph";
import { AgentMark } from "../../components/AgentMark";
import { api, type RemoteDevice } from "../../api";
import { errorText } from "../../utils";

/// 一台设备的表单草稿：从列表数据初始化，保存时整体提交；密码留空表示
/// 保留后端已存的（编辑态占位符展示掩码提示）。
type DeviceDraft = { host: string; port: string; username: string; password: string };

function draftOf(device: RemoteDevice): DeviceDraft {
  return { host: device.host, port: device.port ? String(device.port) : "", username: device.username, password: "" };
}

function formatTime(ms: number): string {
  return ms > 0 ? new Date(ms).toLocaleString() : "";
}

/// 远程设备 tab：列出 zcode 配置过的 SSH 远程设备（探测自
/// ~/.zcode/v2/setting.json），可编辑连接参数并录入密码；「立即同步」
/// 通过 SSH 拉取远程 ZCode 用量库快照并入 ZCode 统计。
export function RemoteDevicesPanel() {
  const [devices, setDevices] = useState<RemoteDevice[] | null>(null);
  const [drafts, setDrafts] = useState<Record<string, DeviceDraft>>({});
  const [busy, setBusy] = useState<Record<string, "saving" | "syncing" | "removing">>({});
  const [notes, setNotes] = useState<Record<string, { kind: "ok" | "error"; text: string }>>({});
  const [confirmingRemoveId, setConfirmingRemoveId] = useState("");
  /// 折叠态按设备 id 记忆；缺省（未出现）即折叠。刷新清单后已展开的
  /// 设备保持展开，新出现的设备折叠。
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [loading, setLoading] = useState(false);
  const loadId = useRef(0);

  const load = useCallback(async () => {
    const id = ++loadId.current;
    setLoading(true);
    setError("");
    try {
      const list = await api.listRemoteDevices();
      if (id !== loadId.current) return;
      setDevices(list);
      setDrafts(Object.fromEntries(list.map((device) => [device.id, draftOf(device)])));
      setBusy({});
    } catch (reason) {
      if (id === loadId.current) setError(`无法读取远程设备列表：${errorText(reason)}`);
    } finally {
      if (id === loadId.current) setLoading(false);
    }
  }, []);

  useEffect(() => { void load(); return () => { loadId.current++; }; }, [load]);

  function patchDraft(id: string, patch: Partial<DeviceDraft>) {
    setDrafts((current) => ({ ...current, [id]: { ...current[id], ...patch } }));
  }

  async function save(device: RemoteDevice) {
    const draft = drafts[device.id];
    const port = Number.parseInt(draft.port, 10);
    if (!draft.host.trim() || !Number.isInteger(port) || port <= 0 || !draft.username.trim()) {
      setNotes((current) => ({ ...current, [device.id]: { kind: "error", text: "主机、端口与用户名都不能为空" } }));
      return;
    }
    setConfirmingRemoveId("");
    setBusy((current) => ({ ...current, [device.id]: "saving" }));
    setNotes((current) => ({ ...current, [device.id]: { kind: "ok", text: "" } }));
    try {
      await api.saveRemoteDevice(device.id, draft.host, port, draft.username, draft.password);
      patchDraft(device.id, { password: "" });
      await load();
      setNotes((current) => ({ ...current, [device.id]: { kind: "ok", text: "已保存连接信息" } }));
    } catch (reason) {
      setNotes((current) => ({ ...current, [device.id]: { kind: "error", text: errorText(reason) } }));
    } finally {
      setBusy((current) => {
        const next = { ...current };
        delete next[device.id];
        return next;
      });
    }
  }

  async function sync(device: RemoteDevice) {
    setConfirmingRemoveId("");
    setBusy((current) => ({ ...current, [device.id]: "syncing" }));
    setNotes((current) => ({ ...current, [device.id]: { kind: "ok", text: "" } }));
    try {
      const result = await api.syncRemoteDevice(device.id);
      await load();
      setNotes((current) => ({
        ...current,
        [device.id]: { kind: "ok", text: result.inserted > 0 ? `同步完成：新入库 ${result.inserted} 条用量` : "同步完成：无新增用量" },
      }));
    } catch (reason) {
      setNotes((current) => ({ ...current, [device.id]: { kind: "error", text: errorText(reason) } }));
    } finally {
      setBusy((current) => {
        const next = { ...current };
        delete next[device.id];
        return next;
      });
    }
  }

  /// 只对「已在 zcode 移除」的设备开放：清掉 AMC 侧的凭据、主机密钥
  /// 指纹与本地快照；已归档的用量统计保留。两击确认，第二次才执行。
  async function remove(device: RemoteDevice) {
    setBusy((current) => ({ ...current, [device.id]: "removing" }));
    setConfirmingRemoveId("");
    setNotes((current) => ({ ...current, [device.id]: { kind: "ok", text: "" } }));
    try {
      const message = await api.removeRemoteDevice(device.id);
      await load();
      setNotice(message.message);
    } catch (reason) {
      setNotes((current) => ({ ...current, [device.id]: { kind: "error", text: errorText(reason) } }));
    } finally {
      setBusy((current) => {
        const next = { ...current };
        delete next[device.id];
        return next;
      });
    }
  }

  return <>
    {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button aria-label="关闭错误提示" onClick={() => setError("")}><Glyph name="close" size={16} /></button></div>}
    {notice && <div className="alert alert-success" role="status"><Glyph name="check" size={18} /><span>{notice}</span><button aria-label="关闭成功提示" onClick={() => setNotice("")}><Glyph name="close" size={16} /></button></div>}
    <div className="settings-toolbar">
      <p className="settings-hint">zcode 的远程 SSH 设备在此同步用量（远程数据只存于远程机，AMC 经 SSH 拉取快照）；密码仅保存在本机。</p>
      <button className="button button-muted" disabled={loading} onClick={() => void load()}><Glyph name="refresh" size={16} />刷新</button>
    </div>
    {!devices ? <div className="loading-panel" role="status"><span className="spinner" />正在读取远程设备…</div> :
      devices.length === 0 ? <div className="path-card"><div className="about-note"><Glyph name="shield" size={16} /><span>未探测到 zcode 的远程设备；在 zcode 里连接远程设备后，这里会自动列出。</span></div></div> :
      devices.map((device) => {
        const draft = drafts[device.id] ?? draftOf(device);
        const state = busy[device.id];
        const note = notes[device.id];
        const confirmingRemove = confirmingRemoveId === device.id;
        const open = expanded[device.id] === true;
        const syncLine = [
          device.lastSyncMs > 0 ? `上次同步 ${formatTime(device.lastSyncMs)}` : "",
          device.lastSyncMs > 0 && device.lastInserted > 0 ? `新入库 ${device.lastInserted} 条` : "",
        ].filter(Boolean).join(" · ");
        return <section className="path-card" key={device.id}>
          <button
            type="button"
            className="path-card-head device-card-head"
            aria-expanded={open}
            title={open ? "折叠" : "展开"}
            onClick={() => setExpanded((current) => ({ ...current, [device.id]: !current[device.id] }))}
          >
            <span className={`device-chevron${open ? " open" : ""}`} aria-hidden="true"><Glyph name="arrow" size={15} /></span>
            <AgentMark agent="zcode" active />
            <h3>{device.displayName || device.id.slice(0, 8)}</h3>
            {device.gone
              ? <span className="tag tag-muted">已在 zcode 移除</span>
              : <span className={`tag ${device.installState === "installed" ? "tag-good" : "tag-muted"}`}>{device.installState === "installed" ? "已安装 host" : device.installState || "未知状态"}</span>}
            {device.hostKeyPinned && <span className="tag tag-muted" title="已记录主机密钥指纹，变更会被拒绝">已固定主机密钥</span>}
            <span className="path-count">
              {device.osUser ? `${device.osUser}@${device.hostname || device.host}` : device.id.slice(0, 8)}
              {!open && syncLine && ` · ${syncLine}`}
            </span>
          </button>
          {open && <>
            {!device.gone && (device.lastSeenVersion || device.lastOnlineAt > 0) && <div className="device-meta">
              {device.lastSeenVersion && <span>zcode host {device.lastSeenVersion}</span>}
              {device.lastOnlineAt > 0 && <span>上次在线 {formatTime(device.lastOnlineAt)}</span>}
              {syncLine && <span>{syncLine}</span>}
            </div>}
            <div className="device-form">
              <label>主机
                <input value={draft.host} placeholder="主机名或 IP" onChange={(event) => patchDraft(device.id, { host: event.target.value })} />
              </label>
              <label>端口
                <input value={draft.port} inputMode="numeric" placeholder="22" onChange={(event) => patchDraft(device.id, { port: event.target.value.replace(/[^\d]/g, "") })} />
              </label>
              <label>用户名
                <input value={draft.username} placeholder="SSH 登录用户" onChange={(event) => patchDraft(device.id, { username: event.target.value })} />
              </label>
              <label>SSH 密码
                <input type="password" value={draft.password} autoComplete="off" placeholder={device.passwordHint ? `留空则保留现有密码（${device.passwordHint}）` : "输入 SSH 密码"} onChange={(event) => patchDraft(device.id, { password: event.target.value })} />
              </label>
            </div>
            <div className="device-actions">
              <button className="button button-primary" disabled={state !== undefined} onClick={() => void save(device)}>{state === "saving" ? "保存中…" : "保存"}</button>
              <button className="button button-muted" disabled={state !== undefined} onClick={() => void sync(device)}>{state === "syncing" ? "同步中…" : "立即同步"}</button>
              {device.gone && <button
                className={`button ${confirmingRemove ? "button-danger" : "button-danger-ghost"}`}
                disabled={state !== undefined}
                title="删除 AMC 保存的该设备凭据、主机密钥与快照；已归档的用量统计保留"
                onClick={() => {
                  if (confirmingRemove) void remove(device);
                  else setConfirmingRemoveId(device.id);
                }}
              >{state === "removing" ? "移除中…" : confirmingRemove ? "确认移除" : "移除"}</button>}
              {note?.text && (note.kind === "error"
                ? <small className="subscription-form-error" role="alert">{note.text}</small>
                : <small className="device-note-ok" role="status">{note.text}</small>)}
            </div>
          </>}
        </section>;
      })}
  </>;
}
