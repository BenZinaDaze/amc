import { Glyph } from "./Glyph";

/// 全局错误 / 成功提示条（页头与内容之间），由各页面在页头下渲染。
export function Alerts({ error, notice, onErrorClose, onNoticeClose }: {
  error: string;
  notice: string;
  onErrorClose: () => void;
  onNoticeClose: () => void;
}) {
  return <>
    {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button aria-label="关闭错误提示" onClick={onErrorClose}><Glyph name="close" size={16} /></button></div>}
    {notice && <div className="alert alert-success" role="status"><Glyph name="check" size={18} /><span>{notice}</span><button aria-label="关闭成功提示" onClick={onNoticeClose}><Glyph name="close" size={16} /></button></div>}
  </>;
}
