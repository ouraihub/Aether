# 11 详细设计：aether-providers

> 依赖：aether-support。不依赖任何其他 aether 模块。
> 蓝本：AutoNote `desktop/src-tauri/ai/src/{probe,classify,url}.rs`、`core/src/ai.rs`（存储、三态密钥）；agent_installer `src-tauri/src/probe/client.rs`（错误提示）。蓝本只作参考实现，不保留其 DTO 与角色表。

## 1. 边界

- 管：服务商 → 线路 → 已选模型的增删改查；凭据加密与三态更新；获取模型列表；按模型名推断能力元数据；把 `ModelRef` 解析成后端连接快照。
- 不管：**角色与回落**（"对话用哪个模型""写作没配就用对话模型"是应用策略，由 `KitHost::select_model` 决定，应用自己存偏好）；发起对话；生成第三方 CLI 配置。
- 协议：`anthropic-messages`、`openai-chat`、`openai-responses`。其他协议名（如某 CLI 的 `openai-completions`）是应用写配置时的映射，不进枚举。
- 思考能力由 `infer_reasoning(model_id, protocol, max_output)` 在探测、列表、model_options、resolve一致推断；已有null自动生效、不写库迁移，非空手工声明优先。当前保守支持Claude预算/adaptive与OpenAI o系列/GPT5族，协议不兼容或未知模型保持未知；实际强度只在聊天时调整。Cherry的registry补能力是交互依据，不移植整套目录。
- 不做：代理、自定义请求头、每线路超时、预置服务商、多密钥轮询、用量统计、无差别重试。

## 2. 数据模型（`providers.db`，migration 1）

```sql
CREATE TABLE providers (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE TABLE endpoints (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
  slug TEXT NOT NULL, base_url TEXT NOT NULL,
  protocol TEXT NOT NULL CHECK (protocol IN ('anthropic-messages','openai-chat','openai-responses')),
  key_ciphertext BLOB, position INTEGER NOT NULL,
  UNIQUE (provider_id, slug)
);
CREATE TABLE models (
  endpoint_id TEXT NOT NULL REFERENCES endpoints(id) ON DELETE CASCADE,
  model_id TEXT NOT NULL,
  model_type TEXT NOT NULL CHECK (model_type IN ('chat','image','video','tts','stt','embedding','rerank','unknown')),
  type_source TEXT NOT NULL CHECK (type_source IN ('inferred','user')),
  context_window INTEGER, max_output INTEGER,
  can_vision INTEGER NOT NULL DEFAULT 0,
  supports_tools INTEGER,                  -- NULL = 未知
  position INTEGER NOT NULL,
  PRIMARY KEY (endpoint_id, model_id)
);
```

- 上限：服务商名 1~80 字符；每服务商 ≤ 20 条线路；每线路 ≤ 1000 个模型；`slug` 1~40 个 `[a-z0-9-]`；`base_url` ≤ 2048。
- 没有 `roles` 表。应用若要"记住对话默认模型"，自己存一个 `ModelRef`；模型被删后 `resolve` 返回 `not_found`，由应用提示重新选择。

## 3. DTO（`ts-rs` 生成；请求 `deny_unknown_fields`）

```ts
type Protocol = 'anthropic-messages' | 'openai-chat' | 'openai-responses'
type ModelType = 'chat' | 'image' | 'video' | 'tts' | 'stt' | 'embedding' | 'rerank' | 'unknown'
interface Model { id: string; modelType: ModelType; typeSource: 'inferred' | 'user'; contextWindow: number | null;
  maxOutput: number | null; canVision: boolean; supportsTools: boolean | null; reasoning: ReasoningConfig | null; nativeWebSearch: boolean }
interface Endpoint { id: string; slug: string; baseUrl: string; protocol: Protocol; hasKey: boolean; keyMasked: string | null;
  keyReadable: boolean; probeUrl: string; models: Model[] }
interface Provider { id: string; name: string; enabled: boolean /* 关闭 = 模型不出现在选择器且不能运行 */; endpoints: Endpoint[] }
interface EndpointSave { id?: string; slug: string; baseUrl: string; protocol: Protocol; apiKey?: string; models: Model[] }
interface ProviderSave { id?: string; name: string; enabled?: boolean /* 缺省 = 保持；新建默认 true */; endpoints: EndpointSave[] }
interface ModelRef { endpointId: string; modelId: string }
interface ModelOption extends ModelRef { label: string /* "服务商 · 线路 · 模型" */; modelType: ModelType; supportsTools: boolean | null; reasoning: ReasoningConfig | null; nativeWebSearch: boolean }
interface ProbeResult { probeUrl: string; models: (Model & { group: string | null })[]; error: ProbeError | null }
interface ProbeError { kind: 'missing_key' | 'unauthorized' | 'not_found' | 'rate_limited' | 'status' | 'timeout' | 'connect' | 'blocked_address' | 'invalid_body';
  status: number | null; message: string | null /* 上游原文，已打码，≤300 字符 */; hint: string | null /* i18n key */ }
```

nativeWebSearch默认false，用户显式声明；只允许chat模型与Messages/Responses协议，保存拒绝Chat Completions声明。搜索优先路由见17；不凭兼容端点模型名字猜测原生搜索。ModelSave的reasoning可省略/nullable，其他字段按生成DTO提交。

providers.db保留migration1原始建表、migration2新增reasoning；migration3新增native_web_search（NOT NULL DEFAULT 0）。2026-10-14修复：首个原生搜索版本误改migration1而保留user_version=2，导致旧库模型列表/能力/连接查询缺列。打开时按实际列区分两种v2，经既有升级事务/备份升至v3；已包含列的v2只推进版本，保留原生声明。不得删库、清空模型/密钥或用空列表吞掉SQL错误。回归覆盖两种v2、重启、模型/思考参数/密文保留。

- `keyMasked` 为首 4 尾 4 打码；`keyReadable=false` 表示密文存在但无法解密（换了 Windows 账户）。
- 保存是**整服务商替换**：事务内按 `ProviderSave` 重写线路与模型，请求里没有的线路被删除。已有线路必须带回 `id`。
- `apiKey` 三态见 00 §7.3；Set 时加密失败 → 整个保存失败，原数据不变。

## 4. Rust API

```rust
pub struct Store { /* Mutex<Connection>；探测用默认 AddressPolicy（providers 无内网授权字段） */ }
impl Store {
    pub fn open(root: &Path) -> Result<Self, KitError>;
    pub fn list(&self) -> Result<Vec<Provider>, KitError>;
    pub fn save(&self, input: ProviderSave) -> Result<Vec<Provider>, KitError>;
    pub fn delete(&self, id: &str) -> Result<Vec<Provider>, KitError>;
    pub fn model_options(&self, filter: ModelFilter) -> Result<Vec<ModelOption>, KitError>; // 给模型下拉
    pub fn resolve(&self, model: &ModelRef) -> Result<Connection, KitError>;
    pub fn endpoint_connection(&self, endpoint_id: &str) -> Result<Connection, KitError>; // 不选模型，供应用写外部配置
    pub async fn probe(&self, target: ProbeTarget, cancel: &CancellationToken) -> Result<ProbeResult, KitError>; // 取消 → cancelled；线路不存在/密钥不可读/地址非法 → Err；上游与网络失败进 ProbeResult.error
}
pub struct ModelFilter { pub model_type: Option<ModelType>, pub supports_tools: Option<bool> }
pub enum ProbeTarget { Saved { endpoint_id: String }, Draft { base_url: String, protocol: Protocol, api_key: KeyChange, endpoint_id: Option<String> } }

/// 后端连接快照：定义在 aether_support（10 文档 §4.1），providers 只负责产出，chat 只负责消费。
pub use aether_support::{Connection, Protocol};
pub fn infer(model_id: &str) -> Inferred;   // { model_type, context_window, max_output, can_vision, supports_tools, group }
pub fn api_base(base_url: &str) -> String;  // 去尾 '/'，未以 /v1 结尾则追加；幂等
```

- `resolve`：模型不存在 → `not_found{entity:"model"}`；密钥缺失返回 `api_key: None`，是否报错由调用方决定。
- `ProbeTarget::Draft` 的 `api_key = Keep` 且给了 `endpoint_id` 时用库里的密钥：用户改了地址还没保存就点"获取模型"也能用旧密钥。

## 5. 获取模型

- `GET api_base + "/models"`；openai-* 用 Bearer；anthropic 用 `x-api-key` + `anthropic-version: 2023-06-01`，同时带 Bearer（不少中转只认 Bearer）。
- 地址先过 `net::check_url`，连接时校验解析地址；被拦截 → `blocked_address`。
- 连接 10s、总 20s、响应体 ≤ 4 MiB、禁止重定向；**不重试**。
- 解析 `{data:[{id}]}`、`{models:[{id}|{name}]}`、`[string]`；去重排序；空列表 → `invalid_body`。
- 错误：无密钥且非回环 → `missing_key`（不发请求）；401/403 `unauthorized`；404 `not_found`；429 `rate_limited`；其他非 2xx `status`；超时 `timeout`；DNS/连接/TLS `connect`；JSON 失败 `invalid_body`。
- `hint`：404 且地址未含 `/v1` → `providers.hint.missing_v1`；`connect` 且主机为 IP → `providers.hint.check_network`；`unauthorized` → `providers.hint.check_key`。
- 探测不写库；用户在 UI 挑选后随 `save` 落库。可取消。

## 6. 推断

- 规则与测试数据：`packages/ui/fixtures/model-classify.json`（Rust 与 TS 共用；放在 npm 包内，宿主经 git 依赖安装后即可读取）。
- 类型：id 含 `embed` → embedding；`rerank` → rerank；`tts`/`speech` → tts；`whisper`/`stt`/`transcri` → stt；`dall-e`/`image`/`flux`/`imagen` → image；`video`/`sora`/`veo` → video；其他 → chat。
- `context_window`/`max_output`/`can_vision`：按家族前缀（claude、gpt、o\d、gemini、deepseek、qwen、glm、kimi、moonshot）查表；查不到为 `null`/`false`。
- `supports_tools`：chat 且家族在表内 → `true`；否则 `null`。
- 用户改过类型（`type_source='user'`）的模型，之后不被推断覆盖。

## 7. 陷阱

1. 保存不带已有线路 `id` → 被当成新线路，旧线路及其模型被删，应用存的 `ModelRef` 失效。UI 必须保留 id。
2. 清空密钥输入框 ≠ 清除密钥：UI 用显式"清除"按钮发 `""`，普通保存不带 `apiKey`。
3. `api_base` 幂等：覆盖 `…/v1`、`…/v1/`、`…/api/v1` 单测。
4. `Connection` 不能出现在日志里：其 `Debug` 已把 `api_key` 显示为 `***`（10 文档 §4.1），不要再为它派生 `Debug`/`Serialize`。
