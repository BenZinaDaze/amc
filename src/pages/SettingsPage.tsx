import { useState } from "react";
import { Alerts } from "../components/Alerts";
import { PageHeading } from "../components/PageHeading";
import { ConfigPathsPanel } from "../apps/settings/ConfigPathsPanel";
import { AboutPanel } from "../apps/settings/AboutPanel";
import type { PageProps } from "./PageProps";

type SettingsTab = "config" | "about";

const settingsTabs: { id: SettingsTab; label: string }[] = [
  { id: "config", label: "配置文件" },
  { id: "about", label: "关于" },
];

/// 设置页：多个 tab 的容器，各面板自行管理数据加载与错误展示。
export function SettingsPage({ error, notice, setError, setNotice, state }: PageProps) {
  const [tab, setTab] = useState<SettingsTab>("config");
  return <>
    <PageHeading title="设置" description="查看 AMC 管理的用户级配置位置与应用信息。" />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    <div className="settings-tabs" role="tablist" aria-label="设置分组">
      {settingsTabs.map((item) => <button key={item.id} role="tab" aria-selected={tab === item.id} className={`settings-tab${tab === item.id ? " active" : ""}`} onClick={() => setTab(item.id)}>{item.label}</button>)}
    </div>
    {tab === "config" && <ConfigPathsPanel />}
    {tab === "about" && <AboutPanel workspacePath={state?.workspace.path} />}
  </>;
}
