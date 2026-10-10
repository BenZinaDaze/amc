import { version } from "../../../package.json";
import { Glyph } from "../../components/Glyph";

/// 关于 tab：版本与工作区信息。
export function AboutPanel({ workspacePath }: { workspacePath?: string }) {
  return <section className="path-card">
    <div className="path-card-head"><Glyph name="shield" size={16} /><h3>关于 AMC</h3></div>
    <div className="path-row">
      <span className="path-kind"><Glyph name="spark" size={15} /></span>
      <span className="path-purpose">应用版本</span>
      <span className="path-value">AMC {version}</span>
    </div>
    {workspacePath && <div className="path-row">
      <span className="path-kind"><Glyph name="folder" size={15} /></span>
      <span className="path-purpose">工作区目录</span>
      <span className="path-value" title={workspacePath}>{workspacePath}</span>
    </div>}
    <p className="about-note"><Glyph name="shield" size={14} />AMC 只写入用户级配置；所有变更都先经过预览确认，原内容保留用于回滚。</p>
  </section>;
}
