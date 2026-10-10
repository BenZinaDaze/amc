import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import type { Update } from "@tauri-apps/plugin-updater";
import { Glyph } from "./Glyph";
import { errorText } from "../utils";

/// 应用更新弹窗：展示版本迁移与更新说明，下载带进度条，安装完成后
/// 重启生效。下载中（busy）不允许 Escape / 点背景关闭，避免装到一半。
export function AppUpdateModal({ update, onClose }: { update: Update; onClose: () => void }) {
  const [progress, setProgress] = useState<number | null>(null);
  const [installed, setInstalled] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const releaseUrl = `https://github.com/BenZinaDaze/amc/releases/tag/v${update.version}`;

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && !busy) onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  async function install() {
    if (busy) return;
    setBusy(true);
    setError("");
    let received = 0;
    let total = 0;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
          setProgress(0);
        } else if (event.event === "Progress") {
          received += event.data.chunkLength;
          setProgress(total > 0 ? Math.min(received / total, 1) : null);
        } else if (event.event === "Finished") {
          setInstalled(true);
          setProgress(null);
        }
      });
      setInstalled(true);
    } catch (reason) {
      setError(`下载更新失败：${errorText(reason)}`);
      setProgress(null);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}>
      <div className="modal update-modal" role="dialog" aria-modal="true" aria-labelledby="app-update-title">
        <div className="modal-head">
          <div><div className="eyebrow">应用更新</div><h2 id="app-update-title">{update.currentVersion} → {update.version}</h2></div>
          <button className="icon-button" aria-label="关闭" disabled={busy} onClick={onClose}><Glyph name="close" size={20} /></button>
        </div>
        <div className="modal-body">
          {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button aria-label="关闭错误提示" onClick={() => setError("")}><Glyph name="close" size={16} /></button></div>}
          <div className="update-notes">{update.body || "本次更新没有附带说明。"}</div>
          {progress !== null && !installed && <div className="update-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(progress * 100)}><span className="update-progress-bar" style={{ width: `${Math.round(progress * 100)}%` }} /></div>}
          <div className="modal-actions">
            <button className="button button-muted" disabled={busy} onClick={onClose}>稍后</button>
            <button className="button button-muted" onClick={() => void openUrl(releaseUrl).catch(() => {})}>查看发布页</button>
            {installed
              ? <button className="button button-primary" onClick={() => void relaunch()}>重启应用</button>
              : <button className="button button-primary" disabled={busy} onClick={() => void install()}>{busy ? (progress === null ? "准备下载…" : `下载中 ${Math.round((progress ?? 0) * 100)}%`) : "下载并更新"}</button>}
          </div>
        </div>
      </div>
    </div>
  );
}
