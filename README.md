# AMC · Agent Manager Center

AMC 是 macOS 桌面端的 Agent 管理工具，集中管理 Skills、MCP，并查看 Agent 用量。

## 安装

从 [GitHub Releases](https://github.com/BenZinaDaze/amc/releases) 下载并安装 macOS 版本。

目前 DMG 未签名。将 AMC.app 复制到 `/Applications` 后，若 macOS 阻止打开，仅在确认下载来源可信时运行以下命令。它会递归清除应用的扩展属性（包括下载隔离标记），不会为应用签名：

```sh
xattr -cr /Applications/AMC.app
```

然后重新打开 AMC。

## 主要功能

- **Agents**：检测本机 OMP；Agents 页汇总所有 Agent 的请求数、Token 总量与已计价费用，并按模型列出明细。
- **订阅与配额**：概览页卡片展示 GLM Coding Plan 的窗口配额与近 24 小时用量，凭据仅在本地使用（详见下文）。
- **Skills**：添加远程 Git 仓库或本地来源，发现并选择安装技能；支持刷新、更新、回滚和卸载。
- **MCP**：查看已发现的服务及配置来源，预览并管理可编辑的 MCP 服务。

## 订阅与配额

概览页的「订阅与配额」卡片通过 Z.ai / 智谱 BigModel 官方监控接口（`/api/monitor/usage/quota/limit` 等）读取 GLM Coding Plan 的配额窗口、MCP 用量和近 24 小时模型用量。

**GLM Key 版本（推荐）**：点击「订阅与配额」右侧的 **＋ 添加套餐** 打开设置弹窗，粘贴 GLM API Key 并选择平台（Z.ai 国际版 / 智谱 BigModel 中国）即可。Key 以 `0600` 权限保存在本机 AMC 数据目录的 `subscriptions.json`，不会离开本机进程；弹窗内可随时保存新 Key 覆盖或清除。概览卡片只做展示，不承载任何操作；AMC 不读取环境变量或其他程序的配置，所有订阅配置都保存在本程序中。只有添加了的套餐才会在概览显示，可随时在弹窗中移除。后续接入其他订阅查询接口时，在 `src-tauri/src/subscription.rs` 的 `providers()` 中追加实现 `SubscriptionProvider` 的供应商即可，前端卡片自动生成。

## 模型定价

Agent 用量的费用估算使用仓库根目录的 `model-pricing.json`（USD / 1M tokens，按模型列出输入、输出、缓存单价与可选的长上下文分档）。Agents 页点击 **刷新状态** 时会从本仓库远程根目录重新拉取该文件：改价格只需修改这一个文件并推送到 GitHub，无需重新安装或发版。

## 界面预览

概览

![AMC 概览页面](docs/images/overview.jpg)

Skills 发现与选择安装

![AMC Skills 发现页面](docs/images/skills.jpg)

安装前的变更预览

![AMC Skill 安装变更预览](docs/images/preview.jpg)

MCP 服务管理

![AMC MCP 服务页面](docs/images/mcp.jpg)
