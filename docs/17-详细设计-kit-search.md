# 17 网络搜索与网页读取

## 1. 范围与归属

`aether-search` 是独立可选 Rust 能力：搜索/读取各选一个默认服务商，每家配置独立保存，DPAPI 密文凭据、地址策略、响应上限和超时沿用 support。聊天核心不读取搜索数据库；`tauri-plugin-aether` 接入层组合搜索和模型请求。

- 搜索：智谱、Tavily、SearXNG、Exa、Exa MCP、博查、Querit、Brave、Parallel、Jina、Firecrawl。
- 读取：Querit、直接获取、Jina、Firecrawl。
- 默认 Exa MCP 搜索、Jina 读取；匿名服务受上游限额影响，不承诺免费额度或可用性。
- 不迁移 Voynote 数据，不实现模型自主多轮检索；显式开启时每轮查询一次用户原话。

## 2. 接口和存储

`Store::open(path, AddressPolicy)` 接收宿主选择的新系统独立数据库路径；旧 schema 返回 `incompatible_db`，不迁移、不读取旧凭据、不删除旧文件。`settings/save/set_default/save_key/remove_key/reveal/save_preferences/preferences/check/search/fetch` 使用 camelCase DTO。

`search_settings(capability,provider)`、`search_providers(capability,provider,base_url,basic_username)` 保存配置；`search_keys(id,capability,provider,key_ciphertext,position)` 每行独立加密，最多16项，每项去首尾空白后1–4096 UTF-16 code units（复用既有凭据加密边界）。列表只返回 `{id,masked}`；更新、删除、显式读取必须同时匹配 capability/provider/keyId。空输入拒绝，不再作为删除操作。`search_preferences` 单行JSON保存全局结果数1–50（默认5）、none/cutoff（默认none）、cutoffLimit 100–20000（默认2000）、最多32条256字符黑名单及模型工具优先（默认false）。配置锁不跨网络；保存失败不写明文或丢旧凭据。

`Builder.search(store)` + `search` Cargo feature 注册 search 命令和 `aether:search` 权限。UI `KitPorts.search` 可选，`createTauriPorts({search:true})` 提供配置、取消、搜索和读取。

共享设置 `SearchSettings` 两张配置卡提供多Key显式增改删/读取与全局偏好。Web 宿主在 SearchPort 设置 `revealNeedsPassword`，验证当前密码后才返回指定Key；Aether 不持有 Web 登录信息。

## 3. AI 对话

输入框工具栏左侧的地球按钮控制联网：正常地球+高亮是开启，斜杠地球是关闭；`aria-pressed` 和双语提示同步变化，运行中禁用。`RunRequest.webSearch` 可省略，默认关闭，仅 chat 允许。

外部路线：接入层接受运行、登记可取消run，再执行预搜索；search_start/search_result驱动状态和来源。失败/停止输出单一终态，不继续请求模型。重新生成查询历史原输入；上下文标记为第三方不可信内容，保留宿主笔记上下文。

原生路线：用户开启联网、SearchPreferences.modelToolsPreferred=true且模型显式声明nativeWebSearch时，跳过外部预搜索，在Responses发送web_search、Messages发送web_search_20250305(max_uses=5)。Chat Completions不接受该声明，未知兼容模型不按名称猜测。服务端工具不进入本地ToolExecutor，正文及HTTP(S)引用转为可点击Markdown；服务端搜索错误保持失败，不静默切provider。原生路线可能发送完整模型请求（会话和附带上下文），UI明确提示；本地黑名单/压缩不冒充控制供应商原生检索。

来源列表只保留当前turn界面状态；历史保存可见回答中的引用，不恢复结构化来源卡片。原生供应商可在一次请求内自主多次检索，框架不额外编排多轮搜索。

## 4. 网络边界

搜索15s、Exa MCP 25s、网页读取20s，共享本次所有Key尝试及响应读取的总期限。每次从下一Key开始轮换，仅明确HTTP401/403/429尝试本次未用过的下一Key；网络/超时/取消/5xx不重试，不跨provider继承或轮换。检测使用同一执行器。成功体最多2MiB、错误体最多8KiB，网页正文保留24000字符安全预算。搜索解析/去重后先黑名单过滤，再按Unicode字符cutoff；none无额外摘要截断。黑名单支持域名及子域、host通配和完整HTTP(S)URL的 `*`/`?` 通配，host不区分大小写、path区分大小写，非法规则整次保存拒绝，不支持正则。认证头敏感且错误脱敏；公开API必须HTTPS，本地开发例外。
读取同样先拒绝黑名单目标URL，直接获取每个重定向目标在请求前检查，返回页再次检查来源URL；cutoff也按Unicode字符处理page.content。域名末尾的DNS根点规范化，不绕过黑名单。

复用 `net::check_url/client` 的 URL/DNS 地址检查，不自动扩大私网权限。供应商请求不跟重定向；直接读取可最多逐跳 5 次 GET，每跳验证并共享总超时，不携带搜索密钥。静态 HTML 提取只去脚本/样式/标签，不执行脚本、不承诺 readability 正文识别。

## 5. 验证记录（2026-10-13）
本节以下为此前单Key系统的历史验收，不代表新合同验收；新多Key/偏好实现补充了可运行的真实HTTP轮换、非重试/取消、作用域隔离、密文/重启、规则及Unicode边界测试，尚待集成门禁执行。


- Rust search 5 项行为测试：所有供应商字段解析、MCP SSE、凭据保持/清除/default/分区及重启、真实本地 HTTP Basic 密码与结果上限、Unicode/网页噪声、超大体提前拒绝；chat 34 项、plugin 16 项通过。
- Rust 三模块 all-target/all-feature Clippy `-D warnings` 通过；已有 ts-rs `deny_unknown_fields` 解析提示仍存在。
- UI lint/typecheck、49 项测试通过，57 个生成 DTO 与 Rust 一致，i18n/模块依赖边界检查通过。
- AutoNote 隔离真实 Tauri/WebView2 窗口：搜索/读取实际匿名 Exa MCP/Jina 返回结果；配置省略密钥保持、清除、结果数和重启读回通过。真实本地 HTTP 服务证明 chat 搜索在模型前、来源可见、笔记上下文保留、搜索失败和停止不发模型请求、关闭联网只发模型。
- 真实付费服务商凭据未联验；协议行为由固定响应与本地请求验证，不把夹具冒充上游联验。
- 使用用户授权的本地 Exa MCP 密钥，通过 AutoNote Worker 实际调用 `https://mcp.exa.ai/mcp`，HTTP 200 返回 Cloudflare 官方文档结果。发现上游已使用 `Highlights:`；两端解析补齐此标记，按 `Title:` 分隔结果而非按空行截断正文。新增多段摘要回归，Rust 搜索 6 项和 Worker 搜索/运行/配置 34 项通过。密钥不写入文档或源码。

## 6. 多Key合同验收（2026-10-14）

- 新schema、每Key密文/作用域、重启读回、401/403/429限定轮换、5xx/网络/取消不轮换、黑名单/重定向目标、Unicode cutoff均已有真实loopback Rust回归；`cargo test --workspace --all-features`退出0；严格workspace Clippy `--all-targets --all-features -D warnings`退出0，ts-rs属性解析提示仍存在。
- UI常显password输入、获取密钥邻标签、图标管理/检测、增改删读/复制、Web密码、默认折叠策略、检测前提交并失败保留草稿。11测试文件53项、typecheck0错误0警告、lint/格式通过；i18n326键、crate依赖13项、64生成DTO一致通过。
- 真实Chromium+AutoNote隔离HTTP/Hono+SQLite读取新合同：Key添加后只显示mask，密码验证显式显示，修改同keyId，Esc关闭后密码/明文清空；高级cutoff100、两条规则、modelToolsPreferred=true实际HTTP读回；刷新Key输入为空、mask保留。320/375/414/768宽无横向溢出。该HTTP宿主使用测试vault，不冒充生产CredentialVault加密；生产加密依赖既有宿主vault验证。
- 原生模型能力显式保存和重启读回；Rust实际HTTP验证Messages/Responses原生工具参数及可点击来源，server_tool_use不走本地执行。Worker原生与外部回落11项定向场景通过。真实供应商原生Key联验未做。

