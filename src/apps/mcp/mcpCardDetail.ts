/// MCP 卡片描述行：URL 剥离查询串/片段与 userinfo（可能携带凭证，保留
/// 「?…」提示有参数被隐藏）；stdio 只显示命令名与参数个数——args 内容
/// 可能内联密钥，与列表 redacted 姿态一致，一律不展示。
export function mcpCardDetail(config: Record<string, unknown>): string {
  const rawUrl = config.url;
  if (typeof rawUrl === "string" && rawUrl) {
    const withoutFragment = rawUrl.split("#", 1)[0];
    const queryStart = withoutFragment.indexOf("?");
    const hasQuery = queryStart !== -1;
    const noQuery = hasQuery ? withoutFragment.slice(0, queryStart) : withoutFragment;
    const schemeEnd = noQuery.indexOf("://");
    let display = noQuery;
    if (schemeEnd !== -1) {
      const authorityAndPath = noQuery.slice(schemeEnd + 3);
      const pathStart = authorityAndPath.indexOf("/");
      const authority = pathStart === -1 ? authorityAndPath : authorityAndPath.slice(0, pathStart);
      const atSign = authority.lastIndexOf("@");
      display = noQuery.slice(0, schemeEnd + 3) + (atSign === -1 ? authority : authority.slice(atSign + 1)) + (pathStart === -1 ? "" : authorityAndPath.slice(pathStart));
    }
    return display + (hasQuery ? "?…" : "");
  }
  const command = config.command;
  if (typeof command !== "string" || !command) return "";
  const argCount = Array.isArray(config.args) ? config.args.length : 0;
  return argCount > 0 ? `${command}(+${argCount} 参数)` : command;
}
