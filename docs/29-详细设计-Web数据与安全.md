# 29 详细设计：Web 数据与安全

> 本文定义 Aether Web Worker 的身份、凭据、授权、数据隔离和安全边界。
> 上位设计：[02-概要设计-Web共享基座.md](02-概要设计-Web共享基座.md)、[22-详细设计-跨运行时公共契约.md](22-详细设计-跨运行时公共契约.md)、[23-详细设计-Web Worker运行时.md](23-详细设计-Web Worker运行时.md)。
> 当前状态：设计稿。生产实现前必须完成部署密钥、身份主体和迁移策略评审。

## 1. 安全目标与威胁模型

### 1.1 保护目标

- 模型、搜索、MCP 和 OAuth 凭据不可被前端、日志、URL、错误或其他主体读取；
- AutoNote 与 Actora 的用户、会话、对话、工具、文件和授权互不越权；
- 工具调用默认最小权限，副作用操作不能因模型输出自动获得授权；
- 浏览器断开、Worker 重启、超时和外部网络不确定时 fail closed；
- 数据删除、撤销和迁移有明确主体与审计结果；
- Worker 与 Desktop 的公共安全语义一致，存储保护实现可不同。

### 1.2 不在 Aether Worker 中解决

- Cloudflare 账户、域名、DNS、WAF 和组织 IAM 的完整运维；
- AutoNote 的笔记内容授权规则；
- Actora 的工作台命令业务授权规则；
- 任意应用的管理员后台和合规保留策略；
- 允许模型自动绕过应用的用户确认。

### 1.3 威胁

| 威胁                  | 防线                                                       |
| --------------------- | ---------------------------------------------------------- |
| Cookie CSRF           | SameSite、Origin/Referer allowlist、CSRF token             |
| XSS 读取 token        | HttpOnly Cookie、严格 CSP、UI 不接收 secret                |
| token 重放            | hash 存储、过期、撤销、audience/application 检查           |
| 跨应用越权            | applicationId 固定、actor/resource policy 双重检查         |
| MCP scope 滥用        | scope 显式授权、工具注册过滤、grant revoke                 |
| OAuth code 劫持       | PKCE、精确 redirect URI、短时一次性 code                   |
| SSRF                  | URL parser、DNS/连接时地址策略、无开放代理、重定向逐跳检查 |
| prompt/tool injection | 外部内容边界、固定工具目录、宿主最终授权                   |
| Worker 重启误执行     | run lease、状态 CAS、unknown 不自动重试                    |
| 日志泄密              | 结构化字段 allowlist、敏感字段禁止记录、脱敏边界           |
| D1 数据误读           | owner/application/tenant 条件必须同时存在                  |

## 2. 主体模型

Aether 只定义最小公共主体，不规定产品资料字段：

```ts
interface Actor {
  readonly subjectId: string
  readonly kind: 'user' | 'service' | 'mcp_client'
  readonly applicationId: string
  readonly tenantId: string | null
  readonly sessionId: string | null
  readonly scopes: readonly string[]
}
```

### 2.1 主体来源

| 来源                   | `kind`       | 用途             | 默认权限                        |
| ---------------------- | ------------ | ---------------- | ------------------------------- |
| Web 登录 session       | `user`       | 浏览器产品 UI    | 产品登录后授予的应用 scope      |
| Worker service token   | `service`    | 受控内部任务     | 显式 audience/scope，无交互权限 |
| MCP OAuth access token | `mcp_client` | 外部 MCP 客户端  | grant 中明确 scope              |
| MCP API key            | `mcp_client` | 外部自动化客户端 | key 创建时明确 scope            |

`applicationId` 由部署组合根固定，不接受客户端自报；同一用户在 AutoNote 与 Actora 的主体是否共享，由产品身份系统决定，Aether 不默认推导跨应用授权。

### 2.2 授权判定

必须同时满足：

```text
有效凭据
  AND 未过期/未撤销
  AND audience/application 匹配
  AND actor 有 scope
  AND 资源属于 actor 可访问主体
  AND 工具最终审批允许
```

任何一项未知或无法读取都拒绝。不要用“同一邮箱”“同一个 display name”或前端传来的 owner 字段替代 subject ID。

## 3. Session

### 3.1 Cookie session

- Cookie 名称由部署配置，不使用产品共享默认名；
- 值为高熵随机 opaque token；数据库只存 hash/HMAC 派生索引，不存明文；
- `Secure`、`HttpOnly`、`SameSite=Lax/Strict` 按登录跳转需求配置；
- 绑定 `applicationId`、subject、createdAt、lastSeenAt、expiresAt、revokedAt；
- 固定空闲过期和绝对过期；刷新生成新 token，旧 token 按旋转规则失效；
- 退出登录撤销当前 session；全局退出撤销 subject 的 session；
- session 不能作为 MCP access token 使用。

### 3.2 Authorization header

- Bearer token 只接受 HTTPS 传输；
- token hash 后查找，比较使用 constant-time；
- header 不进入日志；
- API route 按 token 类型检查 audience、scope 和 rate limit；
- 不在 query、fragment、SSE URL、Referer 中传 token。

### 3.3 CSRF 与 Origin

Cookie 认证的变更请求必须：

1. `Origin` 与部署 allowlist 精确匹配；
2. 非简单请求使用 CSRF token；
3. 服务端比较 token 与 session 绑定值；
4. 缺失或不匹配拒绝，不回显具体失败原因。

MCP HTTP endpoint 使用独立 Origin 策略；不因浏览器 UI 的 CORS allowlist 自动放宽 MCP。

## 4. 凭据与密钥

### 4.1 密钥分类

| 类型                | 例子                          | 存储                                | 前端                               |
| ------------------- | ----------------------------- | ----------------------------------- | ---------------------------------- |
| 上游模型 key        | OpenAI/Anthropic provider key | 加密 ciphertext 或外部 secret vault | 永不返回                           |
| 搜索 key            | Tavily/Exa/Jina               | 加密 ciphertext                     | 只返回 mask/key id                 |
| 外部 MCP secret     | headers/query/env             | 加密 ciphertext                     | 创建/修改时写入，列表只返回 key 名 |
| MCP API key         | 外部客户端 token              | hash + metadata                     | 创建响应只显示一次                 |
| OAuth refresh token | MCP provider token            | 加密 ciphertext                     | 永不返回                           |
| Session token       | 浏览器登录                    | hash/HMAC                           | 仅 HttpOnly Cookie                 |
| Worker master key   | 数据加密根密钥                | Cloudflare Secret/外部 KMS          | 永不返回                           |

### 4.2 加密合同

Aether Worker 提供 `CredentialPort`，Core 不关心具体 KMS：

```ts
interface CredentialPort {
  seal(scope: CredentialScope, value: Uint8Array): Promise<SealedCredential>
  open(scope: CredentialScope, sealed: SealedCredential): Promise<Uint8Array>
  rotate(scope: CredentialScope, sealed: SealedCredential): Promise<SealedCredential>
}

interface CredentialScope {
  readonly applicationId: string
  readonly tenantId: string | null
  readonly subjectId: string | null
  readonly purpose: 'provider' | 'search' | 'mcp' | 'oauth'
  readonly recordId: string
}
```

推荐：

- 使用 AEAD（例如 AES-GCM 或 XChaCha20-Poly1305，由部署实现确定）；
- 每条凭据随机 nonce；
- AAD 至少绑定 application/tenant/subject/purpose/record id；
- ciphertext 带版本和 key id；
- master key 只从 Cloudflare Secret 或外部 KMS 读取；
- 轮换期间支持旧 key 解密、新 key 写回；
- 解密失败返回 `needs_config{reason:"key_unreadable"}`，不返回原始错误和密文；
- 保存失败不覆盖旧 ciphertext。

当前 TS 适配器位于 `packages/worker/src/credentials.ts`：`createCredentialPort({currentKeyId,keys})` 接收显式、不可导出的 AES-GCM CryptoKey；`importCredentialKey` 只导入部署提供的 32-byte base64 secret，没有内置密钥。每条 12-byte 随机 nonce，128-bit tag，AAD 为 `[version,keyId,applicationId,tenantId,subjectId,purpose,recordId]`，旧 key 可读、新 key 写入；单条明文上限 64KiB。`isSealedCredential` 校验未知存储形状；所有解密失败只暴露 `key_unreadable`。

禁止：

- 明文写 D1/KV/R2；
- 用用户密码直接作为数据库加密 key；
- 把 master key 放 `wrangler.toml`、KV、代码或日志；
- 在错误中返回上游 Authorization header；
- 为了“兼容”把 secret 放回 URL query。

### 4.3 API key

创建：

1. 验证 actor 和 requested scopes；
2. 生成高熵 secret 与不可预测 ID；
3. 向用户返回一次完整 token；
4. 只保存 hash、prefix（展示可用，日志不可用）、metadata；
5. 后续列表只返回 id/name/scopes/timestamps/masked prefix；
6. revoke 使用 CAS，重复撤销返回成功但不恢复权限；
7. token 永不过期必须是显式选择并显示风险，默认有 expiresAt。

## 5. OAuth 与 MCP 授权

### 5.1 Grant 对象

```ts
interface GrantRecord {
  readonly id: string
  readonly applicationId: string
  readonly tenantId: string | null
  readonly subjectId: string
  readonly clientId: string
  readonly redirectUri: string
  readonly scopes: readonly string[]
  readonly createdAt: number
  readonly expiresAt: number | null
  readonly revokedAt: number | null
}
```

Grant 的 scope 只允许部署/应用注册过的 scope；请求未注册或超过用户可授范围直接拒绝，不能静默删减后继续授权。

### 5.2 Authorization Code + PKCE

- client registration 限制 redirect URI 精确匹配；
- authorization code 单次使用、短期有效，并绑定 client、redirect、code challenge、actor、scope 和 application；
- token endpoint 必须验证 code verifier；
- access token 和 refresh token 分离保存/轮换，明文只在发放瞬间存在；
- revoke 后 access/refresh/grant 的检查都失败；
- 不把 consent 页面中的真实 secret 回显给前端脚本；
- 错误 redirect 不带 secret、code 或内部详情。

### 5.3 Scope 分层

```text
Aether scope                 # 公共语义
  mcp:access
  web:search
  web:read

Application scope            # 应用注册
  autonote:notes:read
  autonote:notes:write
  actora:commands:invoke
  actora:operations:read
```

Aether 负责 scope 格式、匹配、grant/API key 语义；AutoNote/Actora 负责业务 scope 的注册和执行前资源授权。

## 6. 数据隔离

### 6.1 所有者条件

公共数据记录至少携带：

```text
application_id
tenant_id (可空但必须语义明确)
subject_id / owner_id
created_at / updated_at
```

每次读写都从已验证 `Actor` 注入这些条件，禁止 handler 直接使用请求 body 的 owner/tenant/application 字段。数据库查询必须同时约束 application 和 owner；共享资源使用显式 grant 表，不用 NULL 表示“所有人可见”。

TS `ownerKey(actor)` 固定编码 `[applicationId,tenantId,subjectId]`；D1 每条 SQL 同时约束 application、tenant（JSON 编码，null 非通配符）、subject、owner key、collection 和目标 key。`compareAndSet(null)` 仅创建从未存在的 key；更新和删除是单条 SQL RETURNING CAS，不提供虚假的异步 callback transaction。删除保留 tombstone，旧版本和空版本均不能复活原 key；新建同类资源必须分配新 ID。

`D1DocumentStore.list(owner,collection,{after?,limit?})` 按 key 升序分页，默认/最大 200；需要完整 scope 过滤、TTL 或批量删除的调用方必须遍历页。D1 [单 row/string/BLOB 上限 2,000,000 bytes](https://developers.cloudflare.com/d1/platform/limits/)，因此适配器仅内联不超过 256KiB 的 JSON；更大功能文档使用显式 R2 binding 的不可变、owner-hash 分区 blob + 小型 D1 指针 CAS，默认文档上限 64MiB、部署可配置。没有 R2 时大文档报 `needs_config`，不截断图片或技能。SQL 明确 CAS 冲突可删除新 blob；SQL 结果不确定或旧版本 blob 必须保留给部署显式 GC，避免读者竞态/提交不确定性。文档中的凭据必须已由 CredentialPort 密封。

`R2FileStore` 对 app/tenant/subject 的 owner key 做 SHA-256 分区，只接受受限 ID、配置大小和允许 MIME；immutable conditional put 防覆盖，支持删除和受控 `/api/v1/files/:id` URL，不产生公开 R2 URL。图片校验 MIME signature；技能 JSON/binary 不作图片处理，部署的文件上限必须容纳合法 16MiB 技能包及 base64 开销。

### 6.2 数据归属

| 数据                         | Aether 表/端口 | 应用专属               |
| ---------------------------- | -------------- | ---------------------- |
| Provider/endpoint/model 配置 | 是             | 页面默认值和场景引用   |
| 搜索配置/key                 | 是             | 产品入口和默认策略     |
| MCP API key/grant/OAuth      | 是             | 工具注册和业务授权     |
| Run lease/event/status       | 是             | 产品展示和业务关联     |
| Conversation/turn            | 端口与运行语义 | 表结构、关联、保留策略 |
| Notes/files/shares           | 否             | AutoNote               |
| Apps/commands/operations     | 否             | Actora                 |

Aether 公共表必须有明确 migration owner。AutoNote/Actora 的表迁移不可被 Aether 启动流程偷偷执行。

### 6.3 删除和保留

- 删除主体前先撤销 session、API key、OAuth grant 和 running runs；
- 运行中有外部未知副作用时，记录 unknown 后再删除可删除的元数据；
- 凭据删除使用 tombstone/版本策略，防止旧请求并发写回；
- 产品数据删除由产品定义级联，Aether 只调用产品提供的删除端口；
- 日志默认不保存 prompt/output/token/工具参数；安全审计记录 opaque IDs 和结果码。

## 7. 工具与 MCP 安全

工具执行前必须经过：

```text
工具是否已注册
  → actor 是否拥有 scope
  → 工具是否允许当前 application
  → permissionMode / 默认 approval
  → 宿主 authorize
  → 参数 schema 校验
  → 执行与超时
  → result 限长与外部内容包装
```

规则：

- MCP Server 只暴露显式注册工具；
- `readOnlyHint` 只是提示，不自动授予 auto；
- 工具描述、搜索结果、网页正文和 MCP 返回值都是不可信外部内容；
- 外部内容放入明确边界，不得改变 system rule、scope 或审批；
- 工具参数原文不进日志和普通 UI；审批预览脱敏且限长；
- tool call timeout/连接断开后是 unknown，禁止自动重复副作用调用；
- AutoNote/Actora 工具实现必须自己检查资源权限，不能相信 Aether 已完成所有业务授权。

## 8. 出站网络安全

Worker 所有上游请求通过 `OutboundHttp`：

- provider/search/MCP 的 URL 分别按用途策略校验；
- 拒绝 userinfo、未授权明文 HTTP、loopback/私网/链路本地/未指定地址；
- DNS 解析结果在连接时检查，防止公网域名解析到私网；
- redirect 默认关闭；Fetch 逐跳检查且不携带上游 secret 到新 host；
- 设置 connect/first-byte/idle/total 和响应体上限；
- 供应商 key 只发给配置的 host，不跨 redirect/引用/日志；
- Worker 不能把任意用户 URL 作为开放代理；
- 错误 body 限长、脱敏后才进入诊断。

TS `createWorkerOutbound` 实现 core 的 `fetch(url,init)`，强制精确 origin allowlist、manual redirect、first-byte/idle/total abort 和流式累计字节上限。manual 3xx 返回调用方，由搜索逐跳授权并清除跨 host 凭据；自动跟随的响应拒绝。provider/MCP 不自动跟随。Worker 普通 fetch **不能 pin DNS**，URL/字面 IP 预检不证明真实连接地址；远程出站必须注入声明 `public-addresses-at-connect-time` 的受信 gateway binding，由其真实实现所有 A/AAAA 地址和连接时 rebinding 防护，没有该 binding 则 fail closed。私网 MCP 仅可使用单独 `privateGateway`，精确配置 origin/port 且由受信 gateway 连接时约束；不是全局 allowPrivate。global fetch 仅用于适配器内显式 local-test loopback opt-in。

间接网页代理（Querit/Jina/Firecrawl）的目标还需 `createWorkerTargetCheck` 注入实际 `all-dns-addresses-preflight` checker；无默认 DNS resolver 或语法-only 成功。预检不 pin 代理实际连接，代理也必须实施自身连接时地址策略；这是部署前提，不是假称 Worker 已能保证第三方代理安全。

具体搜索/网页读取约束沿用 [17-详细设计-kit-search.md](17-详细设计-kit-search.md)，Web 端口实现见 [23-详细设计-Web Worker运行时.md](23-详细设计-Web Worker运行时.md)。

## 9. 日志与审计

### 9.1 禁止字段

绝不记录：

- access/refresh/session/API key/OAuth code；
- provider/search/MCP secret；
- Authorization、Cookie 和可重放 header；
- prompt、model output、skill body、完整上下文；
- 完整工具参数、MCP payload、文件正文；
- URL query 中的凭据。

### 9.2 允许字段

```text
requestId, runId, applicationId, opaque subject id,
route, status, error code, durationMs, count,
providerId, endpointId, modelId, tool name (if non-sensitive),
usage availability, event count, state revision
```

`tool name` 如果本身含业务敏感信息则改为注册 ID/hash。日志采用固定字段 allowlist，不接受任意用户对象展开。

TS `createLogger({sink,routes})` 只接收固定事件和字段 allowlist，unknown 字段、prompt、header、URL、嵌套对象全部丢弃；route 必须为预注册模板，不记录实际 URL/query。ID 字段只允许有界内部标识符；调用方不得把正文或 secret 冒充 ID。sink 是部署注入，库不直接 console 输出。

### 9.3 安全事件

只记录必要的安全事件：

- `auth.rejected`；
- `session.revoked`；
- `mcp.grant.created/revoked`；
- `mcp.api_key.created/revoked`；
- `tool.denied`；
- `run.unknown`；
- `credential.decrypt_failed`。

每个事件包含结果和 opaque 关联 ID，不包含失败凭据或用户正文。

## 10. 迁移与密钥轮换

### 10.1 数据库迁移

- migration 版本单调，启动前验证 schema 版本；
- 高于代码的数据库拒绝打开，返回 `incompatible_db`；
- 每一步事务化；失败保留原数据；
- migration 不读取 AutoNote/Actora 旧表、不猜旧字段；
- 产品迁移工具在产品仓库执行，完成后通过新 contracts 接入。

公共表迁移唯一入口为部署显式执行 `packages/worker/migrations/0001.sql`；启动可调用 `validateD1Schema` 只验证版本 1，不创建/迁移表，也不读取产品旧表。R2 blob 回收是部署显式维护操作，不在请求路径猜测活跃引用。

### 10.2 密钥轮换

1. 配置新 master key 和 key id；
2. 新写入使用新 key；
3. 读取旧 key 时按旧 key id 解密；
4. 成功读取后可由后台/显式任务重新密封；
5. 统计未迁移 ciphertext，不删除旧 key；
6. 全部完成并经过恢复演练后撤销旧 key。

轮换失败不能覆盖旧 ciphertext；无法解密的记录显示 `key_unreadable`，不清空、不静默重建。

## 11. 测试和验收

### 11.1 必须有的行为测试

- session/API key/OAuth 过期、撤销、audience/application/tenant 隔离；
- Cookie CSRF、Origin、CORS 和 redirect URI；
- API key secret 只显示一次，数据库/日志无明文；
- provider/search/MCP ciphertext 密文往返、篡改拒绝、旧 key rotation；
- D1 查询无法跨 application/subject；
- scope 未注册、超范围和 revoke fail closed；
- 工具 schema、审批、deny、unknown 不重试；
- SSRF DNS rebinding、redirect、userinfo、私网/loopback；
- body/header/response/tool schema 上限；
- Worker 重启、stale run、取消 race；
- AutoNote 与 Actora 工具、表和日志互不泄漏。

### 11.2 真实场景

1. AutoNote Web：登录用户配置模型和搜索，运行笔记工具，另一个主体读取失败；
2. Actora Web：OAuth/API key 外部客户端只看到授权 scope 内的工作台工具；
3. browser devtools、Worker logs、D1/KV/R2 扫描不到明文 provider/MCP/session secret；
4. 浏览器断开和 Worker 重启后副作用工具不被重复执行；
5. 撤销 session/API key/grant 后已有连接和下一请求都拒绝；
6. 真实部署环境验证 HTTPS、Cookie、CORS、DO、D1、R2 和错误响应。

## 12. 未决项

1. 身份系统由 AutoNote/Actora 各自提供还是共用 issuer；推荐 Aether 只消费标准化 `IdentityPort`，不在基座内绑定产品登录。
2. Worker 密钥由 Cloudflare Secrets、外部 KMS 还是 envelope key service 提供；生产必须支持轮换，不允许代码内固定 key。
3. `tenantId` 是否一期必填；若任一产品支持多租户，公共表从第一版保留列并强制 policy。
4. 审计日志保留期限和导出格式由产品/部署合规决定；Aether 只固定不泄密字段。
5. Web UI 是否允许用户查看当前主体的 API key prefix；推荐允许 mask，不允许显示可重放内容。
