# 13 详细设计：aether-mcp-client 与 aether-mcp-server

> 两个 crate 互不依赖，方向相反：Client 让本程序的 AI **调用外部** MCP 工具；Server 把本程序的能力**开放给外部** AI 客户端。两者都依赖 aether-support，都不依赖 Chat、Skills、Providers。Server 用 `rmcp`；Client 的 Streamable HTTP 直接写在 `aether_support::net`（地址策略、禁止重定向）与 `aether_support::sse` 上，stdio 用 tokio 子进程，均不依赖 `rmcp`（其传输自带 HTTP 客户端，绕过 §3 的地址策略）。握手、分页、调用与结果映射与传输无关，两种传输共用。
> 蓝本：Client 参考 AutoBee `shell/src/llm/{mcp,mcp_url,mcp_external}.rs`；Server 参考 AutoNote `desktop/src-tauri/mcp/src/*`。

# 第一部分：aether-mcp-client

## 1. 边界

- 管：服务配置、凭据、信任、启用；地址策略；Streamable HTTP 与 stdio 两种传输；stdio 子进程的启动、环境与结束；握手；工具发现与调用；**每个工具的审批默认值**；输出与描述限制。
- 不管：聊天循环（Chat）、技能（Skills 不登记 MCP）、业务授权的最终裁决（`KitHost::authorize`）；工作目录是否属于某个工作区（宿主在保存前自行限制）。
- 一期不做 OAuth、resources/prompts/sampling。

## 2. 数据模型（`mcp-client.db`，migration 1–2）

```sql
CREATE TABLE servers (
  id TEXT PRIMARY KEY,
  slug TEXT NOT NULL UNIQUE,              -- [a-z0-9_]{1,24}，工具名前缀 mcp_<slug>_<tool>
  name TEXT NOT NULL, url TEXT NOT NULL,  -- url 不含凭据
  secret_ciphertext BLOB,                 -- secret::seal 的不透明 BLOB，明文为 JSON：{ headers?: {..}, query?: {..} }
  enabled INTEGER NOT NULL DEFAULT 0,
  trusted INTEGER NOT NULL DEFAULT 0,
  allow_private INTEGER NOT NULL DEFAULT 0, -- 用户对该服务显式授权内网地址
  position INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE tools (
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  name TEXT NOT NULL, description TEXT NOT NULL, input_schema TEXT NOT NULL,
  read_only INTEGER NOT NULL DEFAULT 0,   -- 来自 annotations.readOnlyHint，仅作参考
  approval TEXT NOT NULL DEFAULT 'ask' CHECK (approval IN ('auto','ask','deny')),
  fetched_at INTEGER NOT NULL,
  PRIMARY KEY (server_id, name)
);
```

migration 2（stdio）：

```sql
ALTER TABLE servers ADD COLUMN transport TEXT NOT NULL DEFAULT 'http' CHECK (transport IN ('http','stdio'));
ALTER TABLE servers ADD COLUMN command TEXT;  -- stdio：程序路径或 PATH 中的名字
ALTER TABLE servers ADD COLUMN args TEXT;     -- stdio：JSON 字符串数组
ALTER TABLE servers ADD COLUMN cwd TEXT;      -- stdio：绝对目录，NULL = 继承
```

- 已有行保持 `http`。stdio 行 `url` 存空串、`allow_private` 恒为 0；读写都校验列组合，不合法的行按损坏报错。
- stdio 凭据只有 `env`（`{ env?: {..} }`，同样加密、只回显键名）；http 只有 `headers`/`query`。带了另一种传输的凭据 → `bad_request{field:"secret", reason:"transport_mismatch"}`；编辑时改了传输却不替换已存凭据 → `reason:"transport_changed"`。
- 新建服务默认 `enabled=0, trusted=0`。只有 `enabled && trusted` 的服务的工具进入运行目录。
- **信任 ≠ 授权**：每个工具还有 `approval`，默认 `ask`；用户可把只读工具改为 `auto`。外部描述的 `readOnlyHint` 不可信，只用来在 UI 上建议，不自动改为 `auto`。
- 凭据按服务隔离：请求头（如 `Authorization`）和 query 参数（如某些地图 MCP 的 `ak`）都放在加密的 `secret` 中，发请求时拼上，**不落进 `url` 列、不进日志**。这覆盖了"密钥在 URL 里"的服务，不需要宿主改写地址的回调。
- `tools` 表是最近一次成功发现的结果；运行开始时用缓存，缓存超过 10 分钟后台刷新（不阻塞本次运行）。

## 3. 协议与限制

- `POST <url>`，`Accept: application/json, text/event-stream`；SSE 响应用 `aether_support::sse`，取 `id` 匹配的 JSON-RPC 响应。
- 握手：`initialize`（发 `2025-03-26`，接受 `2024-11-05`/`2025-03-26`，其他 → `unsupported`）→ 结果必须含 `protocolVersion` → `notifications/initialized`。
- 会话：有 `Mcp-Session-Id` 则后续带上；404 视为会话过期，重新握手一次。
- 发现：`tools/list` 支持分页，最多 10 页、200 个工具；描述截到 1024 字符；schema ≤ 32 KiB，超出的工具丢弃并记 `warn`。
- 调用：结果 `content[]` 只取 `text` 拼接，其他类型以 `[unsupported content: <type>]` 占位；`isError=true` → `ToolExecution::Error`。
- 超时：连接 10s、单次 60s；响应 ≤ 2 MiB；超时或连接中断发生在请求已发出之后 → `ToolExecution::Unknown`（可能已执行）。
- 地址：`aether_support::net`；`allow_private=1` 时把该服务的 `host:port` 加入策略。禁止重定向。

### 3.1 stdio

- 一次 `McpSession` 内按需启动一个进程，会话结束（drop/取消）即结束；不常驻、不复用到下一次运行。进程意外退出 → 该服务本次报 `server_exited`，下次会话重新启动。
- 启动：tokio `Command` 经 `process-wrap`（`=10.0.1`）包装，参数原样传入、**不经过 shell**。Windows 下裸命令名（无路径分隔符、无扩展名）按子进程环境的 `PATH` 与 `PATHEXT` 查找，使 `npx`、`uvx` 这类 `.cmd` 启动器可用（std 对 `.cmd`/`.bat` 参数做安全转义）；加 `CREATE_NO_WINDOW`。
- 进程树：Windows 把根进程放进 kill-on-close 的 Job Object，Unix 让它自成进程组；后代默认留在其中，根进程先退出也一样。会话结束、取消或断开时同步结束整个 Job Object / 进程组（`TerminateJobObject` / `killpg(SIGKILL)`），回收放到运行时上，drop 不阻塞；Windows 关闭 Job 句柄时也会结束其中全部进程。不调用 `taskkill`、`kill` 等外部命令。用 `process-wrap` 而不自写 Job Object，是因为工作区只允许 DPAPI 一处 `unsafe`。主动脱离 Job 或另建进程组/会话的后代不在此列。
- 环境：清空后复制父进程环境，去掉名字（不区分大小写）以 `_KEY`、`_TOKEN`、`_SECRET` 结尾或含 `PASSWORD` 的变量，再叠加配置的 `env`。同名变量去重，配置值优先；Windows 比较变量名时不区分大小写。程序解析与启动使用这同一份最终环境，配置的 `PATH`/`PATHEXT` 不会被继承值遮蔽。
- 线路：stdout 每行一条 JSON-RPC，单行 ≤ 2 MiB，超过即断开并报错；非 JSON 行忽略（记 `debug`）。stdin 每行一条；请求写入或 flush 失败时整行可能已经送达，按 `Unknown` 处理并结束进程（只有通知的写入失败算确定失败）。stderr 持续读完，防止子进程阻塞；只把前 64 KiB 打码后的摘要记 `info`，**不进错误消息**。
- 超时：启动到 `initialize` 完成 10s；调用与发现沿用上面的限制。
- 找不到程序 → `bad_request{field:"command", reason:"not_found"}`；无执行权限 → `reason:"not_executable"`；其他启动失败 → `spawn_failed`。
- 自调用防护（`own_port`）只对 http 生效。

## 4. API

```rust
pub struct Store { /* Mutex<Connection> */ }
impl Store {
    pub fn open(root: &Path) -> Result<Self, KitError>;
    pub fn list(&self) -> Result<Vec<ServerDto>, KitError>;
    pub fn save(&self, input: ServerSave) -> Result<Vec<ServerDto>, KitError>;
    pub fn delete(&self, id: &str) -> Result<Vec<ServerDto>, KitError>;
    pub fn set_tool_approval(&self, server_id: &str, tool: &str, approval: Approval) -> Result<(), KitError>;
    pub async fn refresh(&self, server_id: &str, cancel: &CancellationToken) -> Result<Vec<ToolDto>, KitError>; // 测试连接 + 更新 tools
    /// 运行开始时调用：返回已启用且信任的服务的工具目录与可复用的连接集合。
    /// `own_port`：本进程正在运行的 MCP Server 端口（接入层在 mcp-server feature 下传入），指向它的服务被跳过（§9.2）。
    pub async fn session(&self, cancel: &CancellationToken, own_port: Option<u16>) -> McpSession;
}
pub struct McpSession { /* 每服务一个连接，按需建立 */ }
impl McpSession {
    pub fn tools(&self) -> Vec<McpTool>;      // { server, slug, name, prefixed, description, schema, approval }
    pub async fn call(&mut self, prefixed: &str, args: Value, cancel: &CancellationToken) -> McpCallResult;
}
```

- `McpTool`/`McpCallResult` 是本 crate 自己的类型；接入层负责把它们转成 Chat 的 `ToolSpec`/`ToolExecution`（Client 不依赖 Chat）。
- 单个服务连接失败只影响该服务的工具（不进本次目录），记 `warn`，不让整次运行失败。

```ts
type McpTransport =
  | { kind: 'http'; url: string /* 不含凭据；列表中打码 */ }
  | { kind: 'stdio'; command: string; args: string[]; cwd?: string /* 绝对目录；缺省=继承 */ }
interface ServerDto { id: string; slug: string; name: string; transport: McpTransport; secretKeys: string[] /* 只给键名 */;
  enabled: boolean; trusted: boolean; allowPrivate: boolean; tools: ToolDto[] }
interface ToolDto { name: string; description: string; readOnly: boolean; approval: 'auto' | 'ask' | 'deny' }
interface ServerSave { id?: string; slug: string; name: string; transport: McpTransport;
  secret?: { headers?: Record<string, string>; query?: Record<string, string>; env?: Record<string, string> } /* 缺省=保持；{}=清空 */;
  enabled: boolean; trusted: boolean; allowPrivate: boolean /* 仅 http 有意义 */ }
```

- `url` 中若带了疑似凭据的 query（`looks_like_secret` 或参数名在打码名单），保存时拒绝并提示移到 `secret.query`。
- stdio：`command` 非空、≤ 1024 字符、单行；`args` ≤ 64 项、每项 ≤ 4096 字符；`args` 在列表中明文展示，看起来像凭据的参数（含 `--flag=value` 的各段）→ `bad_request{field:"args", reason:"credential_arg"}`，应改放 `secret.env`；`cwd` 必须是已存在的绝对目录（`not_absolute`/`not_found`）；`env` 键匹配 `^[A-Za-z_][A-Za-z0-9_]{0,127}$`。

# 第二部分：aether-mcp-server

## 5. 边界

- 管：rmcp Streamable HTTP 服务；默认只监听回环；token 发放、只存哈希、撤销、作用域；只暴露宿主**显式注册**的工具；请求体上限。
- 不管：工具业务实现；幂等去重的业务语义（宿主在工具内实现）；与应用内对话的授权（两者独立：工具在 Chat 中可用**不等于**对外开放）。
- 远程监听（非回环）一期不做；需要时作为新任务，必须同时有 TLS 与认证配置。

## 6. 数据模型（`mcp-server.db`，migration 1）

```sql
CREATE TABLE tokens (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,  -- SHA-256 hex
  scopes TEXT NOT NULL,                     -- JSON 数组，⊆ 宿主声明
  created_at INTEGER NOT NULL, last_used_at INTEGER, revoked_at INTEGER
);
CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1),
  port INTEGER NOT NULL, autostart INTEGER NOT NULL DEFAULT 0,
  enabled_scopes TEXT NOT NULL DEFAULT '[]');   -- 用户总开关
```

## 7. 宿主注册

```rust
pub struct ScopeSpec { pub id: &'static str, pub label_key: &'static str, pub destructive: bool }
pub struct ExposedTool {
    pub name: &'static str, pub description: &'static str, pub input_schema: Value, pub scope: &'static str,
    pub read_only: bool, pub destructive: bool, pub idempotent: bool, pub max_request_bytes: usize, // 默认 64 KiB
    pub handler: Arc<dyn Fn(Value, CallContext) -> BoxFuture<'static, Result<Value, KitError>> + Send + Sync>,
}
pub struct CallContext { pub token_id: String, pub scopes: Vec<String>, pub cancel: CancellationToken }

pub struct McpServer { /* store、tools、运行状态 */ }
impl McpServer {
    pub fn new(root: &Path, config: ServerConfig, scopes: Vec<ScopeSpec>, tools: Vec<ExposedTool>) -> Result<Self, KitError>;
    pub async fn start(&self) -> Result<Status, KitError>;   // 绑定 127.0.0.1:<port>；占用 → conflict{port}
    pub async fn stop(&self) -> Result<Status, KitError>;
    pub async fn set_port(&self, port: u16) -> Result<Status, KitError>;  // 1024..=65535；运行中则重启
    pub fn issue_token(&self, name: &str, scopes: &[&str]) -> Result<IssuedToken, KitError>;
    pub fn revoke_token(&self, id: &str) -> Result<(), KitError>;
    pub fn tokens(&self) -> Result<Vec<TokenRecord>, KitError>;
    pub fn set_enabled_scopes(&self, ids: &[&str]) -> Result<(), KitError>;
    pub fn scopes(&self) -> Result<Vec<ScopeState>, KitError>;  // 声明的 scope + 总开关状态（mcp_server_scopes / _set_scopes 返回）
    pub fn status(&self) -> Status;                          // { running, port, url, toolCount }
}
pub struct ServerConfig { pub default_port: u16, pub token_prefix: &'static str }
```

- 工具用闭包注册而不是 trait：每个工具一个处理函数，宿主可以直接复用自己的业务函数；不需要再实现一个大 trait 做 `match name`。
- 工具名重复、`scope` 未声明 → `new` 返回 `bad_request`。

## 8. 认证与授权

- token：`<prefix>_` + 32 字节 `getrandom` 的 base64url；只存 `sha256`；`IssuedToken.token` 只在创建响应中出现一次。
- 每个请求：对端地址必须是回环；`Origin` 头存在且不是回环来源 → 403（防 DNS 重绑定）；`Authorization: Bearer` 取 SHA-256 后按 `token_hash` 唯一索引查找（256 位随机 token 的摘要，时序最多泄露摘要前缀，无助于求原像，等价于常量时间比较）；未知或已撤销 → 401。
- 调用工具需同时满足：token 含工具的 scope **且** scope 在 `enabled_scopes` 中。否则 JSON-RPC `-32001 forbidden`，记 `warn!(tool, scope, "mcp.server.forbidden")`。
- `tools/list` 只列出当前 token 可调用的工具。
- `last_used_at` 每分钟最多写一次。
- 错误映射：`bad_request` → `-32602`；`not_found` → `-32004`；`conflict` → `-32009`；其他 → `-32603`（message 为安全英文）。

## 9. 陷阱

1. 不要 `bind(0)`：外部客户端配置里的端口会每次变化。用宿主默认端口，占用时让用户改。
2. 同进程同时启用 Client 与 Server 时，Client 的服务地址若指向本进程 Server 端口，`session()` 跳过并 `warn`，避免自调用环路。
3. `destructive` 的 scope 在 UI 上打开时二次确认。
4. rmcp 类型不泄露到公开 API：宿主只看到 `McpServer` 与 `ExposedTool`。
5. Client 侧工具结果的截断在 Chat 做（`tool_output_chars`），这里不重复截断。
