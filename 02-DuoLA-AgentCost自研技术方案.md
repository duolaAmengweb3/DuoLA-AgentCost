# DuoLA AgentCost 自研技术方案

> 实施状态说明：本文是目标与实现约束；具体已完成项、未完成的真实环境门禁和逐条验收记录以 [04-产品级补齐 Todo 与硬验收标准](./04-产品级补齐Todo与硬验收标准.md) 为准。任何“必须”条款都不能仅因写在方案中就视为已通过。

> 文档状态：产品技术基线  
> 版本：3.0  
> 日期：2026-08-13  
> 目标：在不复制 Edgee 私有 Gateway 的前提下，自研一个可安装、可回退、可验证的本地 Agent 成本与上下文控制层。

---
## 0. 先说结论

DuoLA AgentCost 不是“把请求转发一下，再显示一个节省百分比”。它是一层运行时基础设施：

~~~text
用户使用的 Coding Agent
        ↓
DuoLA AgentCost 本地 Gateway
        ↓
用户自己的模型 API / 兼容 Provider
~~~

我们的技术价值必须同时成立：

1. **少传无效上下文**：减少重复、冗余、已经完成的工具输出；
2. **不改变任务语义**：代码、参数、错误、JSON 和用户输入不能被静默改写；
3. **控制失控消耗**：发现重复错误、重复调用、无限重试并及时暂停；
4. **给出真实账本**：以 Provider 返回的 usage 和用户配置的价格计算，而不是凭估算宣传；
5. **可显式退出**：用户可执行 bypass；`launch` 正常退出时恢复配置；不宣称强制杀进程后的自动 Fail Open；
6. **默认不上传内容**：Prompt、代码、工具结果、API Key 留在用户机器。

产品边界：不做：

- 不托管模型；
- 不代理或破解 ChatGPT/Claude 私有订阅接口；
- 不做 TLS 中间人解密；
- 不替用户改代码；
- 不用一个黑盒 AI 对全部上下文做摘要；
- 不把“压缩比例”当成产品结果；当前内置规则包括 ANSI 清理、重复日志折叠、等价 JSON 紧凑化、同请求重复结果引用和保守工具面筛选，仍不冒充语义摘要引擎。

---

## 1. 产品与技术边界

### 1.1 产品职责

AgentCost 负责 Agent 运行时的四类工作：

| 能力 | 解决的问题 | 必须给出的结果 |
|---|---|---|
| 上下文整理 | 同一日志、文件、工具输出被反复带回模型 | 实际发送量、处理规则、原文位置 |
| 工具调用治理 | 重复调用、重复报错、无意义循环 | 循环指纹、次数、暂停原因 |
| 预算控制 | 用户离开后 Agent 继续消耗 | 已用、预计、阈值、暂停或放行 |
| 真实成本账本 | Token 下降不等于账单下降 | Provider usage、价格版本、实际费用 |

### 1.2 不负责的事情

- 不判断代码最终是否正确；它只记录 Agent 的运行证据；
- 不替用户决定模型或供应商；
- 不保证所有客户端内置订阅流量都可被拦截；
- 不在用户项目中写入业务代码；
- 不要求用户把项目上传云端；
- 不把“可能节省”伪装成“已节省”。

### 1.3 正式版本支持级别必须写进产品

| 客户端路径 | 正式版本承诺 |
|---|---|
| Claude Code + API/兼容 Endpoint | 支持 |
| Codex + OpenAI API/兼容 Provider | 支持 |
| OpenCode 等 OpenAI-compatible 客户端 | 支持 |
| Cursor + 用户自带标准模型 Key | 部分支持 |
| Cursor 内置模型、Tab Completion 专用流量 | 不承诺 |
| Codex ChatGPT 订阅私有后端 | 不承诺 |
| Claude Pro/Max 私有订阅后端 | 不承诺 |

客户端支持不是一句“支持 Codex/Claude/Cursor”，而是必须精确到认证路径和协议路径。

---

## 2. 设计原则

### 2.1 本地优先

核心 Gateway、账本、规则和控制器运行在 127.0.0.1。云端只做用户主动开启后的版本发布、匿名遥测或跨设备同步；任何云能力都不能成为本地请求的必经路径。

### 2.2 可显式退出，但不是静默 Fail Open

出现解析失败、规则异常或 SQLite 错误时：

1. 不对未知请求做破坏性修改；
2. 能直接回源就直接回源；
3. 在本地账本记录失败；
4. Dashboard 明确显示“本次未优化”，而不是显示成成功。

如果主进程被强制终止，已设置为本地 Gateway 的客户端请求可能失败；当前产品通过 `bypass` 和正常退出恢复，而不是伪称操作系统级自动 Fail Open。

### 2.3 确定性优先

当前实现只对明确的工具结果和工具定义做确定性处理：ANSI 清理、连续重复长行折叠、等价 JSON 紧凑化、同请求重复结果引用、大工具面任务相关筛选和显式结果上限。代码、命令参数、错误正文和用户文本不做摘要和重写。模型输出只提供用户显式设置的 output token cap，不自动改写。

### 2.4 可逆

每一次转换都生成 receipt：

~~~text
原始内容 hash
转换后内容 hash
规则 ID 与版本
保留/删除的区间
失败回退原因
~~~

默认不保存原文；当前版本 receipt 只能证明 hash、字节数和规则，不提供从账本重建原文的能力。

### 2.5 Provider usage 为准

Token 与费用最终以 Provider 返回值为准。请求估算只用于预算预警，不能用于结算。Provider 没有返回 usage 时，账本标记为 estimated，不能冒充 measured。

账本同时保存 `input_tokens`（本地字节估算）、`measured_input_tokens`、`output_tokens`、`cached_input_tokens` 和 `usage_estimated`。估算与实测字段不能混用，费用计算优先使用 Provider usage。

### 2.6 客户端兼容性优先于压缩比例

任何策略只有在兼容性、正确性和回退测试通过后才可启用。压缩比例不是上线门槛，任务没有被破坏才是。

---

## 3. 总体架构

~~~text
┌───────────────────────────────────────────────────────────┐
│ User Space                                                │
│                                                           │
│  Codex / Claude Code / Cursor / OpenCode                  │
│        │  local env / provider config / relay             │
└────────┼──────────────────────────────────────────────────┘
         ▼
┌───────────────────────────────────────────────────────────┐
│ DuoLA AgentCost Local Runtime                              │
│                                                           │
│  1. Listener & Request ID                                  │
│  2. Protocol Adapter                                       │
│  3. Canonical Request IR                                   │
│  4. Safety Classifier                                      │
│  5. Deterministic Context Rules                            │
│  6. Budget & Loop Controller                               │
│  7. Upstream Transport                                    │
│  8. Stream Encoder                                         │
│  9. Usage Ledger / Receipt Writer                         │
│ 10. Local Admin API                                        │
└────────┼──────────────────────────────────────────────────┘
         ▼
┌───────────────────────────────────────────────────────────┐
│ User-selected upstream                                   │
│ OpenAI API / Anthropic API / compatible endpoint / etc.    │
└───────────────────────────────────────────────────────────┘

┌────────────────────────┐       ┌─────────────────────────┐
│ Local SQLite            │       │ Local Dashboard          │
│ sessions / requests     │◄──────┤ 127.0.0.1 admin UI       │
│ usage / receipts        │       │ status / cost / bypass   │
└────────────────────────┘       └─────────────────────────┘
~~~

### 3.1 进程边界

正式版本只维护两个进程：

1. duola-agentcost：Rust 主进程，负责 Gateway、账本、控制器；
2. duola-agentcost-ui：可选的静态管理页面，优先由主进程提供静态资源，避免额外 Node 服务。

不部署远程数据库、不部署远程模型、不依赖消息队列。单机就能完成核心闭环。

### 3.2 网络边界

默认监听：

~~~text
HTTP: 127.0.0.1:8765
Admin: 127.0.0.1:8766
~~~

只绑定 loopback，不监听 0.0.0.0。若用户主动开启局域网访问，必须显示警告并要求显式配置。

---

## 4. 技术栈与目录

### 4.1 技术选型

| 层 | 选型 | 原因 |
|---|---|---|
| 主运行时 | Rust stable | 单二进制、低内存、跨平台、适合流式代理 |
| 异步运行时 | Tokio | HTTP、SSE、超时、取消和并发控制 |
| HTTP | Axum + Hyper | 轻量路由、流式 body、可测试 |
| 上游客户端 | Reqwest + Rustls | TLS、连接池、超时与代理配置 |
| 序列化 | Serde + serde_json | 协议结构、未知字段保留 |
| 本地账本 | SQLite + WAL | 单机可靠、查询方便、无需外部服务 |
| 日志 | tracing | 结构化事件、敏感字段过滤 |
| CLI | clap | install、doctor、launch、status 等命令 |
| 配置 | TOML + serde | 人类可读、支持 managed block |
| UI | 静态 HTML/TypeScript | 不把 Node 作为生产运行时依赖 |

### 4.2 目录约定

~~~text
agentcost/
├── crates/
│   ├── cli/                 # install / launch / doctor / uninstall
│   ├── gateway/             # listener、route、middleware
│   ├── protocol/            # OpenAI、Anthropic、兼容协议
│   ├── ir/                  # Canonical Request IR
│   ├── transform/           # 确定性规则与 receipt
│   ├── controller/          # budget、loop detector、pause
│   ├── ledger/              # SQLite schema、usage、cost
│   ├── adapters/            # Codex、Claude、Cursor、OpenCode
│   ├── security/            # redaction、权限、敏感字段策略
│   └── testkit/             # fixture、fake provider、fault injection
├── ui/                      # 本地 Dashboard 静态资源
├── fixtures/                # 协议样本和黄金输出
├── migrations/              # SQLite migration
├── docs/                    # compatibility、rules、release
└── Cargo.toml
~~~

模块之间禁止循环依赖：

~~~text
protocol → ir → transform → controller → gateway
ledger    ← gateway/controller/transform
adapters  → protocol + security
cli       → adapters + gateway + ledger
~~~

---

## 5. 统一请求中间表示（Canonical Request IR）

不同客户端和 Provider 的字段不能直接互相转换。必须先进入我们自己的 IR，再由对应 Adapter 编码回去。

### 5.1 Request IR

~~~text
RequestIR {
  request_id: UUID
  session_id: UUID?
  project_id: String?
  client: ClientKind
  protocol: ProtocolKind
  upstream: UpstreamRef
  model: String
  stream: bool
  system: Vec<ContentBlock>
  messages: Vec<Message>
  tools: Vec<ToolDefinition>
  extensions: Map<String, JsonValue>
  budget: BudgetContext
  received_at: Timestamp
}
~~~

### 5.2 Content Block

~~~text
ContentBlock =
  Text { text }
  ToolCall { id, name, arguments_json }
  ToolResult { tool_call_id, name?, content, is_error }
  Image { media_type, source_ref }
  File { name, mime_type, source_ref }
  Reasoning { provider_ref, redacted_text? }
  Unknown { provider, raw_json }
~~~

关键规则：

- Unknown 字段必须保留，不能因为我们的 IR 不认识就丢失；
- 工具参数保留原始 JSON 字节和解析后的结构；
- 图像和文件只保留引用、类型、大小和 hash，不在默认账本里存二进制；
- reasoning 内容不做猜测性重写；
- extensions 存放 Provider 特有字段，Adapter 必须能原样回写。

### 5.3 Adapter 接口

~~~text
trait ProtocolAdapter {
    fn detect(headers, body_prefix) -> DetectionResult;
    fn decode(request) -> Result<RequestIR, DecodeError>;
    fn encode_request(ir) -> Result<UpstreamRequest, EncodeError>;
    fn decode_stream(event) -> Result<StreamDelta, StreamError>;
    fn encode_stream(delta) -> Result<ClientEvent, StreamError>;
    fn extract_usage(response_or_stream) -> UsageStatus;
    fn classify_error(response) -> ProviderErrorClass;
}
~~~

实现顺序：

1. OpenAI Responses；
2. Anthropic Messages；
3. OpenAI Chat Completions；
4. OpenAI-compatible 变体；
5. Vertex/Bedrock 等原生协议。

---

## 6. 请求完整生命周期

~~~text
接收请求
  ↓
生成 request_id / session_id
  ↓
识别客户端和协议
  ↓
读取并校验 body（有上限）
  ↓
decode → Request IR
  ↓
安全分类：可处理 / 必须原样 / 未知
  ↓
执行确定性规则
  ↓
生成 transformation receipt
  ↓
预算预检查与循环控制
  ↓
encode → 上游请求
  ↓
流式转发上游响应
  ↓
提取 provider usage / error
  ↓
写入 usage ledger 和 receipt
  ↓
返回客户端
~~~

### 6.1 请求阶段的硬约束

- 请求 body 默认上限 32 MB，可按项目配置；响应（普通和流式）默认上限 64 MB，可按项目配置；超限直接返回明确错误，不截断后伪装成功；
- 超限不截断，直接原样回源或给出明确错误；
- 不在日志中写完整 Authorization、API Key、Cookie；
- 不为计算压缩率而默认复制完整 Prompt；
- 从第一个字节开始支持流式响应，不能等完整响应才返回；终止事件和 usage 使用有界尾部扫描，不因响应超过 2 MB 而提前判定不完整；
- 客户端取消请求时，立即取消上游请求并记录 cancelled_by_client。

### 6.2 失败路径

| 失败点 | 行为 |
|---|---|
| 协议识别失败 | 原样回源，receipt 标记 protocol_unknown |
| IR 解析失败 | 原样回源，禁止变换 |
| 规则执行失败 | 当前请求原文回源，记录规则异常 |
| 账本写入失败 | 不阻塞请求，内存计数并异步重试 |
| 上游超时 | 转发明确错误，不能自动重复可能有副作用的请求 |
| 流中断 | 不重放已发送的半流式请求 |
| AgentCost 进程退出 | 客户端恢复原始 Provider 配置，或由用户手动 bypass |

---

## 7. 产品级上线能力

正式上线版本一次性包含以下三条核心链路。它们是完整产品的核心闭环，不是对外拆分的功能阶段：

### 7.1 确定性工具结果整理（当前已实现范围）

当前实现范围：

- 明确标识的工具结果中的 ANSI 控制符；
- 连续重复长行折叠，并保留重复次数标记；
- 等价 JSON 紧凑化；
- 同一请求内重复工具结果改为 hash-backed 引用；
- 大工具面在任务明确时做保守相关性筛选，核心代码/文件/终端工具保留；
- 用户显式配置的工具结果字节上限；
- 为每次变换生成原始/结果 hash、字节数和规则 ID。

精确缓存、priority/cost 路由、Token/美元预算、限流和并发上限是 Gateway 控制层能力；缓存默认关闭，只允许安全的非流式、无工具、无执行标记 JSON 请求。

不处理：

- 代码正文；
- JSON 任意字段；
- 命令参数；
- 错误正文；
- exit code；
- 文件路径和行号；
- 用户 Prompt；
- system message；
- 工具调用参数；
- 未识别的 Provider 扩展字段。

### 7.2 预算与循环检测

每次请求计算：

~~~text
request_fingerprint = hash(agent_session + provider + method + path + query + raw_request_body)
~~~

当同一 fingerprint 在窗口内重复超过阈值时：

~~~text
继续第 1～N 次 → 允许
超过阈值       → 暂停并告诉用户
~~~

暂停必须能由用户一键恢复，不能删除上下文或伪造成功。

Session/Daily Token 与 USD 预算都会在真正发往 Provider 前做并发 reservation；请求结束、客户端取消或上游失败后自动释放未结算的 reservation，避免多个并发请求同时通过静态预检查而共同超预算。

### 7.3 真实账本

记录“优化前估算”和“Provider 实际返回”两个维度：

~~~text
estimated_input_tokens
measured_input_tokens
output_tokens
cached_input_tokens
tool_call_count
retry_count
latency_ms
provider_cost
agentcost_processing_ms
~~~

如果 Provider 不返回 input usage，则显示“未提供实际输入 Token”，不显示假精确数字。

---

## 8. 不做黑盒摘要的原因

把整段工具输出交给另一个模型总结，会增加：

- 额外 Token 和费用；
- 额外延迟；
- 摘要遗漏风险；
- 数据离开本地的风险；
- 结果不可复现；
- 失败时难以证明是模型还是规则造成的。

因此，正式版本的默认“节省”来自结构化去重和循环控制，而不是额外调用一个 AI。任何未来加入的本地模型摘要，也必须：

1. opt-in；
2. 内容默认不出本机；
3. 保留原文引用；
4. 通过任务回归集；
5. 不把摘要结果当作事实账本。

---

## 9. 规则引擎：安全、可解释、可回退

### 9.1 三类处理状态

每个内容块在进入规则引擎后必须先分类：

| 状态 | 含义 | 是否允许改变 |
|---|---|---|
| SAFE_DETERMINISTIC | 已有规则证明不会改变语义 | 允许 |
| PRESERVE_EXACTLY | 代码、参数、错误、JSON 等关键内容 | 不允许 |
| UNKNOWN | 新协议、未知字段、无法识别的输出 | 不允许 |

规则引擎只能接收 SAFE_DETERMINISTIC 内容。任何分类不确定都进入 PRESERVE_EXACTLY。

### 9.2 规则定义

规则不是散落在代码里的字符串替换，而是有版本、有测试样本的声明式对象：

~~~text
Rule {
  id: "tool-result.ansi.v1"
  version: 1
  input_kind: ToolResult
  preconditions: [...]
  operation: RemoveAnsiControl
  invariants: [...]
  rollback_on: [...]
  evidence_fixture: "fixtures/ansi/*.json"
}
~~~

每条规则必须声明：

- 适用的内容类型；
- 必须满足的前置条件；
- 允许删除的最小区间；
- 必须保持的字段；
- 不通过校验时的回退行为；
- 对应黄金样本。

### 9.3 规则执行算法

~~~text
for block in request.blocks:
    class = safety_classifier(block)
    if class != SAFE_DETERMINISTIC:
        keep_original(block)
        continue

    candidate = rule.apply(block)
    if invariant_check(block, candidate) == PASS:
        emit(candidate)
        receipt.add(block.hash, candidate.hash, rule.id)
    else:
        emit(block)
        receipt.add_rollback(rule.id, reason)
~~~

### 9.4 不允许的优化

以下逻辑不得作为默认规则上线：

- 通过正则删除看起来像代码的行；
- 按长度截断未知 JSON；
- 删除错误堆栈中的中间行；
- 合并不同工具调用的结果；
- 改写 Agent 的工具参数；
- 自动把结果改成“摘要”；
- 依赖远程 AI 判断什么是重要信息。

### 9.5 版本和灰度

每条规则独立开关，配置文件中保存：

~~~text
rules:
  tool-result.ansi.v1: enabled
  tool-result.pagination.v1: enabled
  tool-result.duplicate-wrapper.v1: observe
~~~

新规则先进入 observe，只计算“如果启用会减少多少”，不改变真实请求；通过回归集和真实 A/B 后才变为 enabled。

---

## 10. 账本与数据模型

### 10.1 数据保存原则

默认保存元数据，不保存原始业务内容：

- 保存 hash、长度、类型、位置和规则版本；
- 不保存完整 Prompt；
- 不保存完整 Completion；
- 不保存 API Key、Cookie、OAuth token；
- 不保存文件二进制；
- 原文缓存尚未实现；在实现前，产品只承诺 hash/规则 receipt，不承诺从账本恢复原文。

### 10.2 SQLite 表

#### sessions

~~~text
id                  TEXT PRIMARY KEY
project_id          TEXT
client              TEXT NOT NULL
model_provider      TEXT
model               TEXT
started_at          INTEGER NOT NULL
ended_at            INTEGER
status              TEXT NOT NULL
original_config_ref TEXT
~~~

#### provider_profiles

~~~text
id                  TEXT PRIMARY KEY
name                TEXT NOT NULL
protocol            TEXT NOT NULL
endpoint            TEXT NOT NULL
model_map_json      TEXT NOT NULL
credential_ref      TEXT NOT NULL
route_policy_json   TEXT NOT NULL
enabled             INTEGER NOT NULL
created_at          INTEGER NOT NULL
updated_at          INTEGER NOT NULL
~~~

credential_ref 只能指向环境变量或系统密钥链引用，不能写入明文 Key。route_policy_json 保存用户显式配置的 fallback/reroute 条件和允许的模型范围。

#### requests

~~~text
id                    TEXT PRIMARY KEY
session_id            TEXT NOT NULL
sequence_no           INTEGER NOT NULL
protocol              TEXT NOT NULL
method                TEXT NOT NULL
path                  TEXT NOT NULL
request_hash          TEXT NOT NULL
input_bytes           INTEGER NOT NULL
estimated_input_tokens INTEGER
output_tokens         INTEGER
status                TEXT NOT NULL
failure_class         TEXT
started_at             INTEGER NOT NULL
finished_at            INTEGER
latency_ms             INTEGER
~~~

#### usage_records

~~~text
request_id          TEXT PRIMARY KEY
source              TEXT NOT NULL  -- provider / estimated
input_tokens        INTEGER
output_tokens       INTEGER
cached_tokens       INTEGER
reasoning_tokens    INTEGER
price_version       TEXT
input_cost          REAL
output_cost         REAL
total_cost          REAL
currency            TEXT
~~~

#### transformation_receipts

~~~text
id                  TEXT PRIMARY KEY
request_id          TEXT NOT NULL
block_path          TEXT NOT NULL
original_hash       TEXT NOT NULL
result_hash         TEXT NOT NULL
original_bytes      INTEGER NOT NULL
result_bytes        INTEGER NOT NULL
rule_id             TEXT NOT NULL
rule_version        INTEGER NOT NULL
status              TEXT NOT NULL  -- applied / rollback / observed
removed_ranges_json TEXT
created_at          INTEGER NOT NULL
~~~

#### loop_events

~~~text
id                  TEXT PRIMARY KEY
session_id          TEXT NOT NULL
fingerprint         TEXT NOT NULL
kind                TEXT NOT NULL
count_in_window     INTEGER NOT NULL
threshold           INTEGER NOT NULL
action              TEXT NOT NULL  -- allow / warn / pause
created_at          INTEGER NOT NULL
~~~

### 10.3 数据库可靠性

- SQLite 使用 WAL；
- 每次写入使用事务；
- 账本写失败不阻塞模型请求；
- 启动时执行 migration，版本不匹配则停止写入但仍可 bypass；
- 数据库损坏时导出诊断信息，禁止自动删除用户数据；
- 用户可以执行 export、backup、clear 三个显式命令。

### 10.4 费用计算

价格配置必须版本化：

~~~text
PriceTable {
  provider: "example"
  model: "example-model"
  version: "2026-08-13"
  input_per_million: 1.00
  output_per_million: 4.00
  cached_input_per_million: 0.10
  currency: "USD"
}
~~~

费用：

~~~text
total_cost =
  input_tokens / 1_000_000 * input_price
  + output_tokens / 1_000_000 * output_price
  + cached_tokens / 1_000_000 * cached_price
~~~

价格过期时显示“按旧价格估算”，不覆盖历史账单。

---

## 11. 预算、重试与循环控制

### 11.1 预算层级

预算按以下层级计算，近的优先：

1. 单请求；
2. 单任务/Session；
3. 当前项目；
4. 当日；
5. 当月。

配置示例：

~~~text
budget:
  request_usd: 1.00
  session_usd: 10.00
  project_usd: 50.00
  daily_usd: 20.00
  max_same_error: 3
  max_retries: 2
~~~

### 11.2 预算动作

| 阈值 | 动作 |
|---|---|
| 70% | Dashboard 和 stderr 提醒 |
| 90% | 下一次请求前二次确认 |
| 100% | 暂停新请求，允许当前流结束 |
| 超出 | 用户显式恢复，记录 override |

不能在请求已经发给 Provider 后才判断是否超预算；需要使用最近窗口的成本估计做预检查。

### 11.3 重试分类

只对明确可重试的失败重试：

- 429；
- 连接建立失败；
- 部分 5xx；
- 上游明确标记 transient。

不自动重试：

- 401/403；
- 参数校验错误；
- 模型不存在；
- 用户取消；
- 已经开始返回内容的流式请求；
- 具备外部副作用但没有幂等键的请求。

每次重试都必须生成新 request 记录，并与原请求关联，不能把多次调用伪装成一次。

### 11.4 循环指纹

归一化参数时只处理无语义的格式差异：

- JSON key 排序；
- 空格和换行；
- URL 尾部斜杠；
- 明确的分页游标；
- 时间戳字段单独标记。

不能删除可能影响调用语义的参数。指纹只用于提示和暂停，不用于改写请求。

---

## 12. 协议与客户端适配

### 12.1 OpenAI Responses

必须支持：

- 非流式与 SSE；
- response.created、response.output_text.delta、response.function_call_arguments.delta 等事件；
- tool call 的 id、name、arguments；
- previous response 关联；
- usage 在末尾事件或响应中的不同位置；
- unknown event 原样转发；
- 取消时上游连接关闭。

### 12.2 Anthropic Messages

必须支持：

- message_start、content_block_start、content_block_delta、content_block_stop、message_delta、message_stop；
- text、tool_use、tool_result；
- cache read/write usage；
- stop_reason；
- beta header 和未知字段；
- 非流式响应。

### 12.3 OpenAI Chat Completions

兼容：

- messages；
- tools/tool_choice；
- function_call；
- choices delta；
- finish_reason；
- usage；
- response header。

### 12.4 兼容协议的处理

兼容 Provider 不能只按 URL 判断。检测顺序：

1. 用户显式配置的 protocol；
2. Content-Type；
3. path；
4. body 字段；
5. 无法确定则原样转发。

### 12.5 客户端 Adapter

#### Codex

通过用户配置的 model provider 指向本地 URL，保留原 Provider 的认证方式和模型名。安装器必须：

- snapshot 原始配置；
- 写入带有 DuoLA 标记的 managed block；
- 只改指定 Provider；
- 启动前 doctor；
- uninstall 时只删除我们写入的区块；
- 发现文件被用户改动时不覆盖，提示人工处理。

#### Claude Code

API 路径通过 ANTHROPIC_BASE_URL 和用户 API Key 接入。启动器不读取或上传 key，只负责将环境变量指向 localhost；未设置 API Key 时直接给出可操作提示。

#### Cursor

只为用户自带 API Key 的标准模型提供适配。MCP 配置是另一条独立能力，不将 MCP 代理宣称为模型流量代理。Cursor 内置模型不纳入正式版本承诺。

#### OpenCode

按 OpenAI-compatible base URL 适配；如果 Provider 使用非标准字段，进入 Unknown 保留路径。

#### 通用兼容入口

提供一个明确的 OpenAI-compatible endpoint，但标注：

- 只保证协议兼容；
- 不保证客户端所有高级功能；
- 未测过的字段只能保证原样透传。

#### 本地 BYOK 路由

正式版本支持用户在本机配置多个 Provider，但不托管或转移用户的模型 Key：

- 每个 Provider profile 保存 endpoint、协议、模型映射和 Key 引用；
- Key 只从环境变量、系统密钥链或用户原有 Agent 配置读取；
- 用户可以配置默认 Provider 和有序 fallback；
- 当前版本支持用户显式配置的简单 `model_map`（incoming model → upstream model）；它不是按价格自动选择模型的 route DSL；
- Fallback 只发生在收到上游首字节之前；
- 只对连接失败、明确的 429、部分 5xx 和 Provider 标记的 transient 错误触发；
- 已经开始返回内容的流式请求不得切换 Provider 重放；
- 401、403、参数错误、模型不存在和用户取消不得自动切换；
- 每次路由决策、失败原因和最终 Provider 都写入账本；
- 不允许为了“更便宜”偷偷更换模型，模型切换必须来自用户显式策略。

这提供的是本地 BYOK 默认 Provider + 受限 fallback，不是 DuoLA 提供模型额度，也不产生 DuoLA 的 Token 账单。

---

## 13. 配置注入、恢复与卸载

### 13.1 配置状态机

~~~text
ORIGINAL
  ↓ install
SNAPSHOTTED
  ↓ write managed block
MANAGED
  ↓ launch
ACTIVE
  ↓ stop / crash / uninstall
RESTORING
  ↓ verify
ORIGINAL 或 CONFLICT
~~~

### 13.2 原子写入

所有配置变更：

1. 读取原文件；
2. 校验语法；
3. 生成快照；
4. 写临时文件；
5. fsync；
6. rename 替换；
7. 重新读取并校验；
8. 记录 config_version。

进程在写入中途崩溃时，临时文件不得成为有效配置。

### 13.3 恢复规则

- 只删除标记为 DuoLA managed 的区块；
- 用户在区块内修改过内容时，显示 diff，不自动覆盖；
- 保存恢复前后的 hash；
- 不能因为卸载 AgentCost 删除用户原有 Provider；
- bypass 命令必须随时可用，不依赖网络。

### 13.4 CLI 命令

~~~text
duola-agentcost install
duola-agentcost doctor
duola-agentcost launch codex
duola-agentcost launch claude
duola-agentcost launch cursor
duola-agentcost status
duola-agentcost stats --today
duola-agentcost bypass
duola-agentcost restore
duola-agentcost export --output path
duola-agentcost uninstall
~~~

命令输出必须说明：当前 Agent、当前 Provider、Gateway 状态、是否优化、失败时如何退出。

---

## 14. 隐私和安全边界

### 14.1 默认不做的事情

- 不安装根证书；
- 不读取浏览器 Cookie；
- 不抓取 OAuth token；
- 不解密任意 HTTPS；
- 不上传代码和 Prompt；
- 不保留完整 API Key；
- 不把请求转发到 DuoLA 的远程模型。

### 14.2 本地访问控制

- Admin API 仅监听 loopback；
- 对管理操作要求随机本地 token；
- token 存储权限为用户可读写；
- macOS/Linux 配置和账本目录权限为 0600/0700；
- Windows 使用用户 ACL；
- 日志中的 Authorization、Cookie、API key 只显示前缀和 hash。

### 14.3 诊断包

用户执行 doctor 或导出诊断包时，默认只包含：

- AgentCost 版本；
- OS/架构；
- Provider 名称和协议；
- 失败类型；
- 延迟和状态码；
- 规则版本；
- hash 和大小。

不包含：

- Prompt；
- Completion；
- 代码；
- 工具结果原文；
- API Key；
- 项目路径中的完整用户名。

---



## 15. 流式传输、超时和资源控制

### 15.1 流式原则

AgentCost 必须边收边发：

~~~text
上游 event
  → decode stream delta
  → 记录必要 metadata
  → encode 原协议 event
  → 立即写给客户端
~~~

不能为了统计完整 response 而把整段输出先缓存在内存。每个连接拥有独立的上限：

- header 超时：10 秒；
- 首字节超时：60 秒；
- 空闲流超时：120 秒；
- 最大连接时长：按配置，默认 30 分钟；
- 单请求内存预算：16 MB metadata，不含内核 socket buffer。

超过上限时，返回明确错误并记录 timeout_phase。

### 15.2 连接池

- 上游连接按 provider、host、认证配置分组；
- 禁止不同用户配置共享 Authorization；
- keep-alive 默认开启；
- DNS 解析失败和 TLS 失败分类记录；
- 用户设置 HTTPS proxy 时，仅上游连接走 proxy，Admin API 仍仅 loopback。

### 15.3 取消与背压

客户端关闭连接后（当前实现）：

1. 取消对应 upstream future；
2. 不继续消费上游数据；
3. 写入 `cancelled` / `cancelled_by_client` 事件；
4. 释放 session、并发槽位和 USD 预算预留。

下游发送缓慢时使用有界 channel，不能无界缓存完整模型输出。

### 15.4 重启恢复

Gateway 重启不恢复正在进行的模型请求。运行中的请求标记 interrupted，客户端可按照 Agent 自己的语义重试；AgentCost 不伪造一个完成响应。

---

## 16. 本地 Dashboard 和用户交互

Dashboard 是观察和控制面，不承载模型调用。

### 16.1 首页必须回答的问题

1. AgentCost 当前是否接管请求；
2. 当前 Agent 和 Provider 是什么；
3. 今天真实花了多少钱；
4. 今天少发送了多少内容；
5. 有多少请求被原样放行；
6. 有没有预算告警或重复错误；
7. 出问题怎样立即 bypass。

### 16.2 页面结构

~~~text
状态
  ├── 当前运行状态
  ├── 当前连接
  └── 一键 bypass / restore

成本
  ├── 今日 / 项目 / 会话
  ├── measured 与 estimated 分开
  └── Provider 价格版本

请求
  ├── 请求时间线
  ├── 协议 / 模型 / latency
  ├── 规则 receipt
  └── 原样放行原因

循环与预算
  ├── 重复错误 fingerprint
  ├── 重试次数
  └── 被暂停的请求

设置
  ├── Provider 与本地端口
  ├── 规则开关
  ├── 预算
  ├── 精确响应缓存（默认关闭）
  ├── Token / output cap
  ├── 限流与并发
  └── 数据清理
~~~

### 16.3 结果表达

不能只显示“节省 42%”。必须显示：

~~~text
本次会话
Provider 实际输入：126,400 tokens
Provider 实际输出：8,200 tokens
估算未优化输入：153,600 tokens
规则处理减少：27,200 tokens
真实账单：以 Provider usage 计算
规则回退：1 次
重复错误暂停：0 次
~~~

如果没有 measured usage：

~~~text
Provider 未返回输入 Token，本页只展示估算，不计入“真实节省”。
~~~

### 16.4 不阻碍用户

任何暂停、回退、配置冲突都必须有：

- 原因；
- 影响；
- 下一步；
- “恢复原始 Agent”按钮。

禁止用“优化失败”“服务异常”这种没有行动信息的错误文案。

---

## 17. 测试与验证方案

### 17.1 测试分层

| 层级 | 内容 | 门槛 |
|---|---|---|
| 单元测试 | hash、分类、规则、价格、预算 | 每次提交执行 |
| 协议黄金测试 | 请求/响应/流事件 round-trip | 关键字段 100% 保留 |
| 差分测试 | 直连 Provider 与经 Gateway | 内容语义和状态一致 |
| 故障注入 | 超时、断流、429、5xx、DB 锁 | 不静默丢失、不死循环 |
| 真实 Agent A/B | Codex、Claude Code、OpenCode | 同任务成功率不下降 |
| 长会话测试 | 多轮工具调用、长日志 | 内存和句柄不泄漏 |
| 配置测试 | install、改配置、崩溃、卸载 | 原配置可恢复 |
| 隐私测试 | 日志、诊断包、SQLite | 不出现完整敏感字段 |

### 17.2 协议黄金样本

每个协议至少收集：

- 最小文本请求；
- system + user；
- tool call；
- tool result；
- image/file 引用；
- reasoning/unknown 字段；
- 非流式；
- 流式；
- usage 在不同位置；
- provider error；
- 用户取消。

黄金测试要求：

1. decode → encode 后关键字段一致；
2. 未知字段原样存在；
3. stream event 顺序不改变；
4. usage 不重复计算；
5. receipt 与实际变换一致。

### 17.3 差分测试

对同一任务保存两条路径：

~~~text
Path A: Agent → Provider
Path B: Agent → AgentCost → Provider
~~~

比较：

- Agent 最终退出状态；
- 工具调用序列；
- 工具参数 hash；
- 文件变更；
- 测试结果；
- Provider usage；
- 失败类型；
- 延迟。

不能只比较模型最终文本，因为 Coding Agent 的价值在工具执行和项目结果。

### 17.4 真实任务集

至少包含：

1. 单文件小改动；
2. 跨文件重构；
3. 长日志排查；
4. 测试失败修复；
5. 数据库迁移；
6. MCP 工具调用；
7. 混合 Node/Python 项目；
8. 重复错误；
9. 上游限流；
10. 中途取消和恢复；
11. 大量工具 schema；
12. 长时间无人值守任务。

每个任务固定：

- 仓库 commit；
- 环境镜像；
- Agent 版本；
- Provider/model；
- max budget；
- 任务 prompt；
- 评价脚本。

### 17.5 真实节省验证

必须分开报告：

~~~text
transport_success_rate
semantic_equivalence_rate
provider_usage_measurement_rate
deterministic_reduction_rate
actual_cost_change
loop_pause_precision
false_pause_rate
~~~

“Token 少了”但任务失败，记为失败；“任务成功但 Provider usage 没返回”，不能记作真实节省。

---

## 18. 性能和可靠性目标

以下是上线目标，不是未测试的营销承诺：

| 指标 | 目标 |
|---|---|
| 空闲内存 | macOS/Linux 小于 100 MB |
| 确定性规则额外延迟 | P95 小于 50 ms |
| 未知字段保留 | 100% |
| 工具参数静默改变 | 0 |
| 默认原始内容上传 | 0 |
| 账本失败导致请求失败 | 0 |
| 直连与 Gateway 任务成功率差异 | 不得下降 |
| 流式首字节额外延迟 | P95 小于 100 ms |
| 配置恢复成功率 | 100%（无用户并发修改冲突） |
| 重复错误漏报 | 在固定测试集达到目标阈值 |

若指标未达到，产品显示“实验性”，不能以生产能力宣传。

---

## 19. 版本和发布方案

### 19.1 构建目标

- macOS arm64；
- macOS x86_64；
- Linux x86_64；
- Linux arm64；
- Windows x86_64。

每个 release 提供：

- 二进制；
- SHA-256；
- SBOM；
- 版本变更；
- 兼容性矩阵；
- 回滚说明。

### 19.2 安装方式

优先提供：

~~~text
macOS: Homebrew + 直接安装脚本
Linux: shell installer + tarball
Windows: PowerShell installer
CI: 固定版本下载与 checksum 校验
~~~

安装器只安装本地二进制和配置，不偷偷注册远程服务。

当前仓库提供源码安装脚本；预编译二进制、checksum、SBOM、Homebrew、PowerShell 和跨平台 CI 仍属于发布门禁，不能由本地 macOS 构建结果替代。

### 19.3 更新和回滚

- 新版本先执行 doctor；
- 保存上一版本二进制；
- 规则版本与程序版本分开；
- Gateway 失败自动停止接管；
- 用户可以一条命令回滚程序和规则；
- 数据库 migration 可向前兼容，不能破坏旧版本读取。

### 19.4 兼容性矩阵

每次发布更新：

| Client | Auth path | Protocol | Stream | Tools | Usage | Status |
|---|---|---|---|---|---|---|
| Codex | OpenAI API | Responses | yes | yes | measured | tested/limited |
| Claude Code | Anthropic API | Messages | yes | yes | measured | tested/limited |
| OpenCode | compatible | Chat/Responses | yes | yes | provider-dependent | tested/limited |
| Cursor | BYOK | provider-dependent | TBD | MCP separate | provider-dependent | partial |

---

## 20. 内部开发依赖顺序（不是对外分阶段）

对外发布的是一个完整产品。下面只是工程实现时的依赖顺序，不能被解释成用户只能先购买一个残缺版本：

### A. 基础骨架

- Rust workspace；
- CLI；
- loopback listener；
- health/status；
- SQLite migration；
- structured logging；
- doctor。

验收：没有任何变换时，可以稳定地把一个测试请求流式转发到 fake Provider。

### B. 协议和账本

- OpenAI Responses adapter；
- Anthropic Messages adapter；
- Chat Completions adapter；
- usage extraction；
- request/session/usage 表；
- receipt 表。

验收：黄金样本 round-trip 和流式差分测试通过。

### C. 安全规则

- safety classifier；
- ANSI 清理、连续重复工具日志折叠；
- 显式 opt-in 的工具结果上限（保留头尾并插入标记）；
- exact preservation；
- observe mode；
- rollback receipt。

验收：规则无法改变代码、参数、错误和 JSON；不通过就原样回源。

### D. 控制器

- budget；
- loop fingerprint；
- retry classification；
- pause/resume；
- bypass。

验收：重试和预算都能被真实 Agent 观察到，且用户可恢复。

### E. Client adapters

- Codex provider 配置；
- Claude env launcher；
- OpenCode base URL；
- Cursor BYOK 实验适配；
- snapshot/restore/uninstall。

验收：安装、启动、崩溃、恢复、卸载不破坏用户原配置。

### F. Dashboard 和发布

- 状态页；
- 费用页；
- 请求时间线；
- receipt 详情；
- 诊断包；
- cross-platform build；
- release signing。

验收：陌生用户不看源码也能知道是否接管、花费多少、如何退出。

### G. 当前不纳入正式发布的能力

以下能力不作为本次正式版本承诺：

- 本地 artifact 引用；
- observe-only tool surface；
- 高置信度的工具集合裁剪；
- 用户 opt-in 的本地摘要；
- 可选云端团队账本。

---

## 21. 硬验收门槛

### 必须通过

- [ ] Codex API Provider 流式请求可用；
- [ ] Claude Code API 路径流式请求可用；
- [ ] OpenAI/Anthropic 黄金样本 round-trip；
- [ ] unknown 字段 100% 保留；
- [ ] 工具参数 hash 100% 不变；
- [ ] 代码、JSON、错误、路径和行号不被规则删除；
- [ ] 规则失败自动原样放行；
- [ ] Provider usage 与 ledger 一致；
- [ ] 本地 BYOK Fallback 只在首字节前触发；
- [ ] 已开始流式输出的请求不会被自动重放到其他 Provider；
- [ ] 路由决策、失败原因和最终 Provider 可追溯；
- [ ] priority/cost 路由只在用户显式配置时生效，且同协议、最大尝试次数受控；
- [ ] 精确缓存默认关闭；开启后流式、工具、执行/实时请求不会命中缓存；TTL、LRU 和清空可验证；
- [ ] output token cap 只在用户显式设置时注入，并按协议使用正确字段；
- [ ] 每分钟限流和并发上限达到阈值会记录阻断原因；
- [ ] 估算和实测明确分开；
- [ ] 预算达到阈值会暂停；
- [ ] 重复错误达到阈值会提示或暂停；
- [ ] 用户可一键 bypass；
- [ ] 进程退出后原始 Agent 可恢复；
- [ ] install/restore/uninstall 不覆盖用户修改；
- [ ] 默认诊断包不含 Prompt、代码和 API Key；
- [ ] 真实任务 A/B 成功率不下降；
- [ ] 长会话无内存和文件句柄泄漏。

### 不能用以下方式“通过”

- 只测 fake request，不测真实 Agent；
- 只测 HTTP 200，不测工具调用和流式；
- 只显示估算 Token；
- 用重试掩盖解析失败；
- 把失败请求排除出统计；
- 用摘要结果替代原文验证；
- 声称支持订阅流量但没有官方配置入口；
- 把 Edgee 的公开数字当成我们的测试结果。

---

## 22. 本方案与 Edgee 的关系

我们只借鉴公开的工程事实：

- 不同 Agent 需要不同启动器和 Adapter；
- CLI 与桌面应用的接入路径不同；
- 本地 relay 和配置恢复是可行模式；
- 确定性工具结果 trimming 应有回退；
- benchmark 必须固定任务和环境；
- 开源项目可以作为规则和测试参考。

我们自己实现并负责的部分：

- DuoLA Canonical Request IR；
- Provider Adapter 和流式兼容；
- 本地账本、真实 usage、价格版本；
- safety classifier 和 rule receipt；
- budget、loop detector、pause/resume；
- 配置快照、原子写入、恢复；
- 本地 Dashboard；
- 真实 Agent A/B 与发布门禁。

公开的 Edgee CLI 并不等于其生产 Gateway 完整开源；本项目不复制 Edgee 的私有 Gateway、分类器、计费服务、品牌或内部数据。技术调研和来源见：

- Edgee CLI：[github.com/edgee-ai/edgee](https://github.com/edgee-ai/edgee)
- Edgee 的为什么这样做：[edgee.ai/docs/introduction/why-edgee](https://www.edgee.ai/docs/introduction/why-edgee)
- Edgee 基准工具：[github.com/edgee-ai/compression-lab](https://github.com/edgee-ai/compression-lab)
- Anthropic Gateway 配置：[Claude Code LLM Gateway](https://docs.anthropic.com/en/docs/claude-code/llm-gateway)
- Codex Provider Schema：[Codex config schema](https://github.com/openai/codex/blob/main/codex-rs/core/config.schema.json)

---

## 23. 最终工程判断

这套产品可以落地，但价值不是“加一个代理端口”。它成立的前提是：

1. 至少有两种真实协议可以无损流式转发；
2. 至少有一组确定性规则能在固定任务上减少重复内容；
3. 规则出错时不会损坏 Agent 任务；
4. 预算和循环控制能在真实长任务中阻止无效消耗；
5. 用户能看到真实账单，而不是营销数字；
6. 用户随时可以退出、恢复原配置和删除本地数据。

如果不能通过这些门槛，就不应把产品称为 Agent Cost Infrastructure；最多只能称为实验性代理工具。
