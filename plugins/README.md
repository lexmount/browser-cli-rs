# 云浏览器接入索引

本仓库统一维护 browser-cli、共享 Skill 和客户端包装。按调用方式区分
CLI 与远程 MCP；同一种 Skill 不再按客户端复制维护。

本次是源码归档与候选包构建，**不更新已安装插件，不替换市场包，不发布服务**。
下表是 2026-09-22 整理时的来源记录，不代表实时审核状态。

## 目录与修改入口

```text
skills/
  lexmount-browser/             CLI Skill，保留既有路径
  lexmount-browser-mcp/         远程 MCP Skill，不调用 CLI
plugins/
  assets/                      Codex 包共享图标
  codex/cli/lexmount-cloud-browser/   CLI 插件清单模板
  codex/mcp/lexmount-cloud-browser/   MCP 插件清单与连接配置模板
  claude/lexmount-cloud-browser/     Claude Code 清单模板
scripts/
  package-skill.sh              独立 Skill ZIP
  package-plugins.py            从共享 Skill 生成客户端候选包
```

修改操作说明、认证流程、脚本时改 `skills/`；客户端展示信息与连接声明改
`plugins/`。模板目录没有重复的 Skill，**不能直接作为完整插件安装**，应先打包。
生成包的根目录均为 `lexmount-cloud-browser/`，内部包含清单、共享 Skill 的完整副本
及所需资源，不依赖源码仓库中的相对外部路径或符号链接。

## 各客户端来源与接入边界

| 客户端 / 产物 | 源码或审核入口 | 本次处理 / 后续条件 |
| --- | --- | --- |
| WorkBuddy Skill | [CLI Skill](../skills/lexmount-browser) | 已上架版本保持不变；后续改动从共享 CLI Skill 产生新候选包。 |
| Codex CLI 插件 | [清单](codex/cli/lexmount-cloud-browser/.codex-plugin/plugin.json) · [商店](https://chatgpt.com/plugins/plugins_6aab7b0e7e2481919910e41aa1181a0a) | 汇入 1.1.20 的包装信息；改为共享主线 Skill 后使用 `1.1.21-dev.1`，不是市场原包的字节级副本，须重新验收后发布。 |
| Codex MCP 插件 | [清单与 MCP 配置](codex/mcp/lexmount-cloud-browser) · [MCP Skill](../skills/lexmount-browser-mcp) | 从 `1.2.0-beta.2` 整理为 `1.2.0-beta.2.dev.1` 候选；Skill 标识明确为 `lexmount-browser-mcp`。仍需服务上线与宿主 OAuth、工具调用、接管验收。 |
| OpenClaw 插件 | [PR #33](https://github.com/lexmount/browser-cli-rs/pull/33) · [固定源码](https://github.com/lexmount/browser-cli-rs/tree/6650e9cf29ab5f18d5e74b90e6e5d4cea62cc8cd) · [商店](https://clawhub.ai/tristanisk/plugins/lexmount-cloud-browser) | #33 核对时仍 Open，保留独立审核，不重复导入其安全修复。其 `package.json`、`openclaw.plugin.json` 位于仓库根，入口在 `plugins/openclaw/`，直接引用共享 CLI Skill；合并后沿用该布局。 |
| OpenClaw Skill | [商店](https://clawhub.ai/tristanisk/skills/lexmount-cloud-browser) | 独立 Skill 与原生插件是两种产物；本次不更新既有市场版本，也不迁移其许可承诺。 |
| Hermes Skill | [上游 PR #114031](https://github.com/NousResearch/hermes-agent/pull/114031) | 上游维护 `optional-skills/research/lexmount-cloud-browser`；后续同步共享 CLI Skill，并遵循上游格式要求。该链接表示提交入口，不表示已合并。 |
| Claude Code 插件 | [清单](claude/lexmount-cloud-browser/.claude-plugin/plugin.json) | 汇入原 CLI 包装，接共享 Skill 后版本为 `1.1.18-rc.2`；客户端验收暂停，未作为可发布结果。目标是 Claude Code。 |
| 豆包 Skill | [开发者后台](https://developer.doubao.com/dashboard) | 可准备独立 CLI Skill 包；先前登录后要求企业邀请码，尚未完成市场提交。后台准入与本地安装分开核对。 |
| 千问 Skill | [开放平台](https://open.qianwen.com/home) | 可准备独立 CLI Skill 包；先前页面标注 Skill 接入“即将开放”，未完成公开渠道提交；不能把 Agent 入驻视为 Skill 提审。 |
| DeepSeek Harness 插件 | [lexmount/dsh-browser](https://github.com/lexmount/dsh-browser) | 已有独立仓库，保留原生插件源码与发布流程；本索引聚合入口，不复制一套实现。 |

## 构建与校验

macOS / Linux 上构建独立 Skill（需要 `sh`、`zip`）：

```sh
./scripts/package-skill.sh
./scripts/package-skill.sh lexmount-browser-mcp
```

依次输出 `dist/lexmount-browser.zip` 与 `dist/lexmount-browser-mcp.zip`。
两个 ZIP 均以 `SKILL.md` 为根，不含 CLI 二进制或本机凭据。

构建客户端候选包（开发环境 Python 3.9+ 标准库；不是插件运行时依赖）：

```sh
python3 scripts/package-plugins.py codex-cli
python3 scripts/package-plugins.py codex-mcp
python3 scripts/package-plugins.py claude-cli
python3 tests/test_plugin_packages.py
```

Windows 使用 `py -3` 替代 `python3`。每个 ZIP 旁附 `.zip.sha256`。
包内 `skills/lexmount-browser/` 或 `skills/lexmount-browser-mcp/` 与共享源码逐字节一致，
构建时排除 `bin/`、缓存与符号链接。固定 ZIP 顺序、时间与文件权限便于复核。
本地固定 Python/zlib 环境中的重复打包应字节相同；不同压缩库版本不保证 ZIP 哈希一致。

CLI 与 MCP 两种 Codex 包沿用同一个插件标识，**是替代版本，不应同时安装**。
切换前保留旧版本来源并在隔离开发安装中验收，不自动覆盖市场版。
CLI Skill 保留主线平台支持范围；存在 Linux CLI 二进制不代表当前主线 Skill 的
Linux 引导已完成，相关扩展见 [PR #27](https://github.com/lexmount/browser-cli-rs/pull/27)。

## MCP 服务与发布依赖

远程入口：`https://browser.lexmount.cn/chatgpt-app/mcp`。
`.mcp.json` 只含服务地址与 OAuth resource；不写 client secret、Token、
固定 client_id、猜测的回调地址或本地测试回调。
宿主负责发起授权及保存凭据；固定客户端需登记宿主实际回调，CIMD 路径需独立核验。

| 依赖 | 代码审核入口 |
| --- | --- |
| OAuth 应用与回调配置入口 | [lex-home #352（内部仓库）](https://code.lexmount.net/feixiang/lex-home/pulls/352) |
| MCP 服务及内嵌接管契约 | [lexmount-nodejs-backend #447](https://github.com/lexmount/lexmount-nodejs-backend/pull/447) |
| MCP 测试客户端 | [mcp-test-client #18](https://github.com/lexmount/mcp-test-client/pull/18) |

这些依赖由各自 PR 审核与部署。本仓库合并不等于服务已部署，也不等于客户端已通过验收。
MCP Skill 的 `browser_control`、`browser_view` 与 App 内部 `browser_handoff` 依赖该候选契约。
发布前先按部署版本复核真实工具 Schema，再验证首次授权、网页读取、填写后读回、
登录接管后继续操作及会话清理；保留实际客户端截图。格式与打包检查只能证明包可构建。

## 来源与许可

- Codex CLI 包装来源：2026-09-21 的 1.1.20 修订包；本次使用主线 CLI Skill，旧市场包不变。
- MCP Skill、配置、图标和包装来源：2026-09-22 的 1.2.0-beta.2 开发候选；只调整 Skill 标识与对应默认提示，不宣称部署或验收完成。
- Claude 包装来源：2026-09-16 的 1.1.18-rc.1 CLI 候选；本次仅整理包装并接共享源码。
- 本仓库与新生成包带 [MIT LICENSE](../LICENSE)。ClawHub 独立 Skill 的 MIT-0 承诺仅针对当时提交的 Skill 文件，不能推广到托管浏览器服务。服务使用继续受服务条款约束。
- 不收集本机凭据、客户资料、历史聊天或原始授权日志；不将旧 ZIP、安装缓存和二进制作为第二份源码长期维护。
