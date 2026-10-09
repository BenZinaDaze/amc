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

- **Agents**：检测本机 OMP、Claude Code CLI 与 Codex CLI；Agents 页汇总所有 Agent 的请求数、Token 总量与已计价费用，并按模型列出明细。OMP 用量读取本地 stats.db，Claude Code 用量解析 `~/.claude/projects` 下的会话转录（支持 `CLAUDE_CONFIG_DIR`），同一请求经 resume/重试产生的重复行只计一次；Codex 用量解析 `~/.codex/sessions` 下的 rollout 转录（支持 `CODEX_HOME`），按 `token_count` 事件的每轮增量累计，并跟随会话内 `/model` 切换分别计价。
- **订阅与配额**：概览页卡片展示 GLM Coding Plan 的窗口配额与本周用量（模型用量按点数重置周期统计，接口无周期数据时回退滚动 24 小时），以及 Sub2API 实例的订阅计划与用量（粘贴实例地址 + API Key，读取其 `GET /v1/usage`：订阅模式展示日/周/月窗口、余额模式展示钱包余额、限额 Key 展示总额度与 5h/1d/7d 速率窗口，另附总/今日请求数与 Token、按重置周期对齐（日精度，`start_date` 仅支持日期）的模型分项），凭据仅在本地使用。另支持 Google Antigravity 订阅（Google One AI Pro / Ultra）：点"通过 Google 登录"走浏览器 OAuth 授权（PKCE + 本地回调，无需安装 Antigravity），卡片展示 Gemini 池与非 Gemini 池（Claude/GPT）各自的 5 小时滚动窗口与每周窗口的剩余比例、重置倒计时及套餐层级；授权凭据（长期 refresh token）只保存在本机，额度查询走 Google Cloud Code 内部接口，非官方接口，格式可能随版本变化。DeepSeek API 卡片读取官方 `GET /user/balance`：按币种展示余额（¥/$），赠金与充值进明细，`is_available` 作为可用状态标签。另支持 Cursor 个人订阅（Pro / Pro+ / Ultra）：添加时走浏览器授权（本地生成 PKCE 打开 `cursor.com/loginDeepControl`，后端轮询登录结果，无需安装 CLI 或粘贴 Cookie），卡片展示 Cursor Models / Other Models / Total 三条使用比例与账期重置倒计时、套餐层级与账号邮箱；网页登录 token 约两个月有效，过期后删除该套餐重新登录即可，凭据只保存在本机，额度查询走 Cursor 内部接口（非官方，格式可能随版本变化）。
- **Skills（分发管理）**：添加远程 Git 仓库或本地来源，发现并安装技能。技能记录与中央副本集中保存在 AMC 本地数据库与数据目录（唯一事实来源），用户级技能目录只是投影：`~/.agents/skills`（OMP 的 agents 来源与 Codex 的 USER 级共用，双方默认加载）和 `~/.claude/skills`（Claude Code 个人级）。OMP 与 Codex 各有独立开关：共用目录停用其一时不删文件，而是写入该 Agent 自己的配置——OMP 在 `config.yml` 的 `skills.ignoredSkills` 按名称忽略，Codex 在 `~/.codex/config.toml` 的 `[[skills.config]]` 按路径 `enabled = false`；两者都停用才移除目录。列表中点击开关即生成变更预览，应用前可审阅差异；只覆盖 AMC 当前启用中的投影（内容哈希校验），任何非 AMC 投影的同名目录——包括内容相同的用户副本——一律拒绝触碰。分发条目支持从仓库更新（重新投影启用中的目标）与整体移除。
- **MCP**：统一管理 MCP 服务并同步到各 Agent。服务定义集中保存在 AMC 本地数据库（唯一事实来源），每个服务可勾选写入 OMP（`~/.omp/agent/mcp.json`）、Claude Code（`~/.claude.json`，支持 `CLAUDE_CONFIG_DIR`）或 Codex（`~/.codex/config.toml`，支持 `CODEX_HOME`）的用户级配置；列表中点击 Agent 开关即生成变更预览，应用前可审阅文件差异，写入采用原子替换并自动备份。Agent 未安装时只记录开关、不创建文件。AMC 只管理通过它保存的服务，不读取或合并 Agent 现有配置。

## 界面预览

概览

![AMC 概览页面](docs/images/overview.jpg)

Skills 发现与选择安装

![AMC Skills 发现页面](docs/images/skills.jpg)

安装前的变更预览

![AMC Skill 安装变更预览](docs/images/preview.jpg)

MCP 服务管理

![AMC MCP 服务页面](docs/images/mcp.jpg)
