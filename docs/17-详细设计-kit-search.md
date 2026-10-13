# 17 网络搜索与网页读取

## 1. 范围与归属

`aether-search` 是独立可选 Rust 能力：搜索/读取各选一个默认服务商，每家配置独立保存，DPAPI 密文凭据、地址策略、响应上限和超时沿用 support。聊天核心不读取搜索数据库；`tauri-plugin-aether` 接入层组合搜索和模型请求。

- 搜索：智谱、Tavily、SearXNG、Exa、Exa MCP、博查、Querit、Brave、Parallel、Jina、Firecrawl。
- 读取：Querit、直接获取、Jina、Firecrawl。
- 默认 Exa MCP 搜索、Jina 读取；匿名服务受上游限额影响，不承诺免费额度或可用性。
- 不迁移 Voynote 数据，不实现模型自主多轮检索；显式开启时每轮查询一次用户原话。

## 2. 接口和存储

`Store::open(path, AddressPolicy)` 接收宿主选择的独立数据库路径。`settings/save/set_default/reveal/check/search/fetch` 返回 camelCase DTO。

`(capability, provider)` 是配置主键。凭据字段省略保持、空串删除、非空加密替换；切默认不动别家的密钥，结果数 1–50 默认 5。配置锁不跨网络；保存失败不写明文或丢旧凭据。

`Builder.search(store)` + `search` Cargo feature 注册 search 命令和 `aether:search` 权限。UI `KitPorts.search` 可选，`createTauriPorts({search:true})` 提供配置、取消、搜索和读取。

共享设置 `SearchSettings` 两张配置卡支持默认选择、保存、密钥三态/撤销/显示、检测、结果数，另有真实搜索和 URL 读取。Web 宿主在 SearchPort 设置 `revealNeedsPassword`，验证密码后才返回密钥；Aether 不持有 Web 登录信息。

## 3. AI 对话

输入框工具栏左侧的地球按钮控制联网：正常地球+高亮是开启，斜杠地球是关闭；`aria-pressed` 和双语提示同步变化，运行中禁用。`RunRequest.webSearch` 可省略，默认关闭，仅 chat 允许。

接入层先接受运行、登记可取消 run，再执行预搜索。事件 `search_start/search_result` 驱动状态和当轮来源链接。搜索失败/停止输出单一终态并更新历史，不继续请求模型。重新生成查询历史原输入。搜索输出使用外部上下文边界、编号引用说明和既有 Unicode 字符预算，保留宿主笔记上下文。

来源列表只保留在当前界面的 turn 状态；会话重启仍有模型回答中的引用，但不会恢复结构化来源卡片。未实现的工具式多轮检索不得用此开关冒充。

## 4. 网络边界

搜索 15s、Exa MCP 25s、网页读取 20s，包括响应读取。成功体最多 2MiB、错误体最多 8KiB；摘要 1000 字符，正文 24000 字符。认证头敏感且错误脱敏；公开 API 必须 HTTPS，本地开发例外。

复用 `net::check_url/client` 的 URL/DNS 地址检查，不自动扩大私网权限。供应商请求不跟重定向；直接读取可最多逐跳 5 次 GET，每跳验证并共享总超时，不携带搜索密钥。静态 HTML 提取只去脚本/样式/标签，不执行脚本、不承诺 readability 正文识别。

## 5. 验证记录（2026-10-13）

- Rust search 5 项行为测试：所有供应商字段解析、MCP SSE、凭据保持/清除/default/分区及重启、真实本地 HTTP Basic 密码与结果上限、Unicode/网页噪声、超大体提前拒绝；chat 34 项、plugin 16 项通过。
- Rust 三模块 all-target/all-feature Clippy `-D warnings` 通过；已有 ts-rs `deny_unknown_fields` 解析提示仍存在。
- UI lint/typecheck、49 项测试通过，57 个生成 DTO 与 Rust 一致，i18n/模块依赖边界检查通过。
- AutoNote 隔离真实 Tauri/WebView2 窗口：搜索/读取实际匿名 Exa MCP/Jina 返回结果；配置省略密钥保持、清除、结果数和重启读回通过。真实本地 HTTP 服务证明 chat 搜索在模型前、来源可见、笔记上下文保留、搜索失败和停止不发模型请求、关闭联网只发模型。
- 真实付费服务商凭据未联验；协议行为由固定响应与本地请求验证，不把夹具冒充上游联验。
- 使用用户授权的本地 Exa MCP 密钥，通过 AutoNote Worker 实际调用 `https://mcp.exa.ai/mcp`，HTTP 200 返回 Cloudflare 官方文档结果。发现上游已使用 `Highlights:`；两端解析补齐此标记，按 `Title:` 分隔结果而非按空行截断正文。新增多段摘要回归，Rust 搜索 6 项和 Worker 搜索/运行/配置 34 项通过。密钥不写入文档或源码。
