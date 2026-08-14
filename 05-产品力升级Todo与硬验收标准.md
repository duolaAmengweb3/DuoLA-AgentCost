# DuoLA AgentCost：产品力升级 Todo 与硬验收标准

> 这不是“再加几个功能”的清单，而是把 AgentCost 做成用户真正愿意长期依赖的产品契约。
>
> 产品目标：**不改变 Agent 的任务结果，只在本地确定性地减少无效上下文、控制 Token 和费用、阻止失控请求，并让每一次处理都能解释、能回退、能对账。**

## 0. 先说结论：现在差的不是 Gateway 能不能转发

当前核心运行时已经能完成：本地接入、请求转发、工具结果确定性整理、预算阻断、Fallback、缓存、取消记账、账本和 bypass。

真正还需要做强的，是四件会直接决定用户是否觉得“有用”的事：

1. **省得准**：不是把字节数变小，而是在长任务、工具密集任务、重复探索中持续减少真正发送给 Provider 的上下文。
2. **不改错**：不破坏工具调用、代码、JSON、流式事件、错误和 Agent 的任务状态；不确定时自动保留原文。
3. **管得住**：预算、重试、并发、Fallback、缓存和模型选择都要有明确边界，不能偷偷换模型、偷偷重试、偷偷超支。
4. **看得懂**：用户不需要研究网关日志，就能知道这次为什么省了、为什么没省、被什么拦住、是否可以继续。

本文件是产品能力的主 Todo。`04-产品级补齐Todo与硬验收标准.md` 继续保留工程与真实环境回归门禁；这些门禁和首次运行体验仍然阻塞面向普通用户的完整公开发布。

本轮已实际落地的能力：TaskRun 任务聚合、显式项目/Agent/会话/模型预算 scope、按 scope 的并发预留与账本统计、Provider 瞬态错误熔断、语义投影护栏、Anthropic 结构化 tool result 文本处理、带副作用工具的保守停裁、静态规则注册表、透传原因、严格隐私模式、敏感错误脱敏、缓存诊断、Dashboard 一键旁路/恢复、任务与 receipt Dashboard、doctor 配置检查。仍标为 `[ ]` 或 `[~]` 的项目没有被本轮代码冒充完成。

---

## 1. 产品契约：做什么与不做什么

### 1.1 用户得到的结果

用户安装 AgentCost 后继续使用原来的 Codex、Claude Code、Cursor 或其他兼容 Agent：

```text
Agent 产生请求
    ↓
DuoLA AgentCost 在本机判断：能否安全整理、是否应该缓存、是否超预算、是否需要绕过
    ↓
用户自己的 Provider 收到请求
    ↓
AgentCost 记录真实 usage、费用、节省量和处理证据
```

用户不需要为每个任务手动写规则，也不需要把 Prompt、代码、API Key 上传给 DuoLA。

### 1.2 默认承诺

- 本地优先、BYOK；DuoLA 不代替用户选择模型，也不把请求转发给第二个 AI 做摘要。
- 只处理能证明安全的字段；没有把握就原样透传。
- 只在 `sent < original` 且结构校验通过时采用优化结果；没有实际收益就不优化。
- 不自动改写模型最终答案、不自动修复代码、不做未经用户同意的语义摘要。
- Token、美元费用、Provider 实测 usage、估算值分开显示，不能用估算冒充账单。
- 任意请求都能 bypass；优化层异常不能伪造“任务已完成”。

### 1.3 明确不做的事

- 不做另一个聊天机器人，不和 Coding Agent 争夺任务决策权。
- 不默认启用语义缓存、相似问题复用或模型摘要；这些会改变语义，除非未来有独立的可验证产品契约。
- 不承诺固定节省百分比；公开数字必须来自同 Agent、同模型、同任务的双跑数据。
- 不把云端多租户、SSO、团队管理冒充本地单用户核心能力。

---

## 2. 产品能力 Todo（按用户价值排序）

状态说明：`[x]` 当前已有；`[~]` 已有基础但要补成产品能力；`[ ]` 待完成。

### P0-A：任务级节省证明（把“省 Token”变成可信产品结果）

**问题**：只显示压缩前后字节数，用户无法判断真实账单是否下降，也不知道一整个任务是否真的少走弯路。

- [x] 记录每次请求的原始估算、发送估算、Provider measured usage、输出 usage、缓存 usage、耗时、重试和状态。
- [x] 增加 `TaskRun` 任务级聚合：同一 Agent 会话中的请求、工具调用、预算事件和最终状态归为一个任务；无显式会话 Header 时按 Gateway 进程自动分组。
- [ ] 支持“原始路径 / 优化路径”双跑报告：同一请求快照、同一 Provider、同一模型，只改变是否经过规则。
- [ ] 计算三种节省并分开显示：
  - 上下文节省：`original_input - sent_input`；
  - Provider 实测节省：原始双跑 measured usage - 优化 measured usage；
  - 金额节省：按用户配置的 Provider 价格计算，未配置价格则不显示金额。
- [ ] 记录“无节省透传”：规则命中但没有变短、结构校验不通过、任务不明确时，明确显示 `pass_through`，不能显示虚假的 0% 优化。
- [x] Dashboard/`GET /api/trends` 提供 24 小时至 366 天的按日请求、发送量、可证明减少量、Provider measured usage、费用和阻断趋势；任务/项目维度仍通过 TaskRun 查询。
- [x] 导出脱敏后的 JSON/CSV 账单；不包含 Prompt、代码和完整工具结果原文。

**硬验收**

- 同一任务双跑时，结果状态、工具调用序列和最终响应语义一致；差异必须可定位到规则 receipt。
- Provider 返回 usage 时，Dashboard 以 measured 为主、estimated 为辅；没有 usage 时明确标记 `estimated`。
- 所有“节省”都能回到请求 hash、规则版本和 Provider usage；不能只给一个不可解释的百分比。
- 任何单个请求没有节省时，产品仍然是正确的：原样发送、显示原因、账本结算不出错。

### P0-B：统一协议 IR 与语义护栏（让兼容性成为产品能力）

**问题**：不同 Provider 的 Chat、Responses、Anthropic Messages、SSE 事件结构不同；只靠字符串替换会破坏工具调用或流式状态。

- [x] 已有 OpenAI Chat/Responses、Anthropic 基础适配和透明流式转发。
- [~] 已建立通用 JSON 语义护栏和结构化 tool result 处理；完整的跨协议 `RequestEnvelope` / `ResponseEvent` IR 仍需继续拆分 Adapter。
- [ ] 每个 Adapter 只允许修改 IR 中列出的安全字段，未知字段保留原样。
- [ ] 统一处理并验证：
  - message 顺序与角色；
  - tool name、tool call id、参数 JSON；
  - response id、finish reason、usage、错误类型；
  - SSE event 顺序、终止事件和中断事件；
  - provider 扩展字段、缓存字段和请求追踪字段。
- [ ] 对协议版本建立 fixture 套件：非流式、流式、工具调用、并行工具、空内容、错误、usage 缺失、超时和中断。
- [ ] Adapter 无法识别请求时进入 `opaque pass-through`，而不是猜测字段。

**硬验收**

- 支持矩阵内的 fixture 经过优化前后，业务字段、工具 ID、顺序、finish reason、错误分类 100% 一致。
- 未知字段、未知协议和未来扩展字段不得被删除；无法解析的请求必须原样透传或明确阻断。
- 流式请求任何情况下只能有一个最终账本状态，不能同时出现 `completed`、`cancelled` 和 `failed`。

### P0-C：工具结果压缩引擎 v2（真正解决长任务浪费）

**问题**：一次请求内的重复内容只是最容易的样本；真实浪费来自多轮工具调用反复携带日志、分页结果、Schema、错误和历史探索。

- [x] ANSI 清理、重复行折叠、等价 JSON 紧凑化、同一请求内重复结果引用、大型工具面保守筛选。
- [x] 现有规则均带版本化 rule id、收益判断、hash receipt 和原样回退；`GET /api/rules` 提供静态 Rule Registry，工具面遇到副作用工具自动停止激进裁剪。
- [ ] 结构化工具结果清理：
  - 去除重复的分页包装、无意义 headers 和 transport metadata；
  - 对数组/对象做稳定字段排序或紧凑化，但不改变数组顺序和业务字段；
  - 折叠重复日志、重复堆栈、重复进度事件，并保留次数、首尾样本和 hash；
  - 对 ANSI、控制字符、空白和终端渲染码做可逆规则处理；
  - 对错误结果保留 error code、message、request id、关键 stack 和原始 hash。
- [x] 跨轮次的安全边界已明确：只对当前请求中已出现原文的重复结果做引用；不会在上下文可能已被裁剪时凭 hash 猜测替换，避免造成不可理解的悬空引用。
- [ ] 分页与历史策略：只在能确认页面已被 Agent 读取并且来源 hash 一致时折叠；不允许把尚未读取的页面当成已读。
- [ ] 二进制、代码、配置、SQL、交易 calldata、签名、私钥、用户指定保留字段默认不压缩。
- [ ] 所有规则支持单条 bypass、会话 bypass、全局 bypass。
- [ ] `max_tool_result_bytes` 继续保持显式配置；没有配置时不截断。

**硬验收**

- 结构化 JSON：字段集合一致、数组顺序一致、数字/布尔/null 类型一致。
- 日志：错误码、时间、request id、关键 stack 和最后一条错误不能丢失。
- 代码/配置/SQL/交易数据：默认原样保留；任何规则都不能修改可执行内容。
- 规则只有在输出更短、校验通过、receipt 写入成功时才生效；任一步失败立即使用原文。
- 在内部工具密集任务集上，目标是中位输入节省达到 15% 以上；这是发布目标，不是对所有任务的宣传保证。没有收益的任务必须稳定透传。

### P0-D：工具面与上下文选择（减少 Agent 的无效选择）

**问题**：一个 MCP Server 带来几百个工具时，真正浪费的不只是结果，还有每轮完整的工具定义。

- [x] 已有任务明确且候选工具不少于 4 个时的保守筛选；任务不明确时全部保留。
- [ ] 建立工具元数据规范：名称、用途、输入 schema、只读/副作用、风险级别、依赖、版本、来源。
- [ ] 只做确定性筛选：根据 Agent 已提供的任务文本和工具声明匹配，不调用第二个模型替用户做猜测。
- [ ] 核心工具白名单：文件、代码、终端、版本控制和 Agent 显式调用的工具永不因筛选被删除。
- [ ] 边界工具保留并标记 `kept_due_to_uncertainty`，让用户知道为什么没有激进裁剪。
- [ ] 生成工具面 receipt：原始数量、发送数量、保留/排除原因、规则版本。
- [ ] 支持按 Agent、项目、MCP Server 单独关闭工具面筛选。

**硬验收**

- 任务含糊、工具数量少、存在副作用工具或元数据不完整时，不裁剪。
- Agent 显式调用的工具永远存在；工具名称、参数 schema、必填字段不能被改写。
- 工具面减少后，Agent 的工具调用成功率和任务结果不得下降；测试失败时自动回退完整工具面。

### P0-E：预算、并发、重试与路由控制（不让 Agent 失控）

- [x] Token/USD 预留、请求/会话/每日预算、最大输出、限流、并发、重复指纹、同协议 Fallback。
- [x] 增加预算作用域：`global → project → agent → session → model`，按最窄显式 scope 优先；ledger 按 scope 独立统计和预留。
- [ ] 预算策略可解释：请求发送前显示预计最大消耗；拒绝时告诉用户是 Token、USD、并发、速率还是循环触发。
- [x] Fallback 增加健康状态与熔断：连续瞬态错误的 Provider 暂停一段时间；冷却后允许下一次请求探测恢复，所有跳过原因写入 attempt receipt。
- [ ] 成本路由必须是显式策略：同协议、同能力、同模型约束内选择；不允许因为便宜而偷偷降级模型或能力。
- [ ] Retry policy 版本化：仅对幂等请求或明确 `Idempotency-Key` 的请求自动重试；每次尝试写入 receipt。
- [ ] 支持“只提醒不阻断”和“硬阻断”两种模式，但默认预算达到上限必须阻断。

**硬验收**

- 并发压力下，所有 scope 的预留总和不能超过上限；不能出现预检查通过后共同超支。
- 默认不能重复发送未知副作用 POST；显式允许时必须在用户可见记录中标注。
- Provider 健康异常时，熔断、候选、恢复探测和最终选择原因均可查询。
- 任何模型映射、Fallback 或成本路由都不能改变用户显式指定的模型能力，除非用户配置了明确映射。

### P0-F：缓存成为可证明的资源优化，而不是猜答案

- [x] 精确缓存默认关闭，拒绝流式、工具、执行标记、随机采样和状态型请求；缓存仍经过预算和循环门禁。
- [x] 缓存命中/未命中、过期、hash 校验失败和容量淘汰可通过 `/api/cache/status` 观察；bypass 原因进入 request reason。
- [x] 缓存命名空间包含 Profile、Provider、模型、路径、请求体和凭证/租户相关 header hash。
- [ ] 每次命中显示避免的上游调用、估算节省和实际 measured usage；缓存没有真实 Provider usage 时不伪造。
- [ ] 增加一致性策略：TTL、手动清理、项目级清理、版本变更失效、响应 hash 校验。
- [ ] 语义缓存保持实验性关闭，不进入默认产品承诺。

**硬验收**

- 不同 Provider、账号、租户、模型和请求体绝不能串缓存。
- 缓存响应必须通过 schema/status/hash 校验；校验失败立即丢弃并回源。
- 缓存命中不能绕过预算、限流、重复请求和权限门禁。
- RSS 在连续写入大响应时受总字节上限约束，不随请求数无限增长。

### P0-G：零摩擦接入与恢复（让用户真的能用）

- [x] `serve`、`launch`、Codex 配置快照恢复和 bypass。
- [x] `duola-agentcost doctor`：检查监听地址、认证约束、Provider URL、Key 环境变量、重复 ID、预算 scope 和数据目录；只做本地检查，不消耗 Provider 额度。
- [ ] `install` 自动探测 Codex/Claude Code/OpenCode/Cursor 可配置入口；每一步都能回滚，不能覆盖用户手改配置。
- [ ] 一条命令生成最小可用配置；没有价格、没有预算、没有优化收益时也能以透明透传模式运行。
- [ ] Gateway 进程异常时，Agent 仍可一键 bypass；恢复失败不反复拉起并制造更多请求。
- [ ] 提供 `status`、`pause`、`resume`、`uninstall` 和 `export`，状态和动作名称用人话表达。

**硬验收**

- 新用户从零到第一次成功请求不超过 3 个用户动作，不要求编辑 TOML。
- 卸载后原 Agent 配置、环境变量和工作流恢复到安装前状态。
- Gateway 未启动、Provider Key 缺失、协议不匹配时，界面给出具体处理动作，不显示模糊的“连接失败”。

### P0-H：Dashboard 以任务为中心，不以网关术语为中心

- [x] 已有基础 Dashboard、账本、状态和 receipt 数据。
- [x] 首页只回答五个问题：
  1. 今天是否省了真实 Token/钱？
  2. 哪个任务最浪费？
  3. 哪些请求被阻断，为什么？
  4. AgentCost 是否改变了任何任务语义？
  5. 现在是否可以安全继续工作？
- [x] 任务列表展示：会话/项目、Agent、模型、请求数、原始/发送 Token、实测 usage、节省、费用、阻断和最终状态。
- [x] 任务详情展示 Provider attempt 与 transformation receipt；请求/规则/响应的更细时间线仍可继续扩展。
- [x] 每条优化显示规则、路径、字节变化和 hash；透传/阻断显示人话原因。
- [x] 增加优化质量指标：应用、透传、语义护栏回退、Provider 错误、阻断和缓存统计。
- [x] 增加 Dashboard/CLI 一键全局旁路和恢复；请求级旁路由 `X-DuoLA-Transform` 提供。全局动作写入可恢复的 Profile bypass 文件，并在 `control_events` 留下审计记录。
- [x] Dashboard 主表和任务卡把状态、变换和阻断原因翻译成人话；规则 ID、Provider attempt 等技术细节只在详情中展开。

**硬验收**

- 用户在 10 秒内能找到最近一次任务的真实节省、最终状态和失败原因。
- “证据不足”“未知错误”“处理失败”不能作为最终解释；必须落到 Provider、预算、协议、规则、网络或用户动作。
- Dashboard 显示的数字与 ledger 可对账，刷新、重启和导出后不漂移。

### P0-I：隐私、可恢复与本地数据治理

- [x] 默认不保存 Prompt、代码和完整工具结果原文；Key 只从环境变量读取；Gateway Token 不转发。
- [x] receipt/ledger 的字段分级：只保存元数据、hash、usage 和原因；不保存 Prompt、代码、完整响应或 Key。
- [x] 默认日志脱敏：Provider 传输错误中的 Authorization、API Key、Cookie、签名、私钥和 URL query secret 不落盘；Prompt、代码和完整响应从不写入 ledger。
- [x] 提供本地保留周期清理、脱敏导出和 Profile 独立数据目录；清理会在一个事务内删除 requests、receipts 和 attempts，避免孤立索引。
- [ ] 每个 Profile 独立数据目录，权限、账本、缓存、bypass 和配置快照互不串用。
- [x] 提供“严格隐私模式”：`privacy set --strict` 禁用缓存并保留最小 receipt；任何隐私模式都不保存原文。

**硬验收**

- 在测试请求中放入假 Key、Cookie、私钥和敏感 query，日志、ledger、receipt、错误页和 Dashboard 均不得出现原文。
- 断电/强杀后再次启动，`running` 只能变为 `interrupted`，不能伪造 `completed`。
- 删除 Profile 后，缓存、账本、快照和导出索引均被清理或明确报告残留原因。

### P1：增强项（不改变 P0 产品契约）

- [ ] 任务级策略模板：只读研究、代码修改、生产排障、数据分析分别提供保守默认值。
- [ ] 用户可视化保留规则：例如“永远保留最近 3 次错误”和“永远不压缩 `src/` 下的代码结果”。
- [ ] 预算预测：基于当前任务已发生的消耗估算是否会超预算，但只做提醒，不用预测结果代替硬门禁。
- [ ] 本地报告分享：只分享脱敏的节省/状态/receipt，不分享 Prompt、代码和 Provider Key。
- [ ] 团队能力：策略下发、项目归因、共享预算、审计和组织权限；必须建立在单机核心稳定之后。

---

## 3. 统一的数据模型（避免功能各做各的）

所有能力都围绕以下对象实现，不能各写一套日志：

```text
TaskRun
  ├─ Session / Project / Agent / Model / Provider
  ├─ RequestAttempt[]
  │    ├─ original_snapshot_hash
  │    ├─ transformed_snapshot_hash
  │    ├─ rules[] + rule_version
  │    ├─ estimated_tokens / measured_usage
  │    ├─ reserved_budget / settled_cost
  │    ├─ provider_status / stream_status
  │    └─ finalization_status
  ├─ ToolEvent[]
  ├─ BudgetEvent[]
  ├─ CacheEvent[]
  ├─ BypassEvent[]
  └─ outcome: completed | failed | blocked | cancelled | interrupted
```

约束：

- 一次 `RequestAttempt` 只能有一次最终结算；
- 所有优化规则必须能从 receipt 回放“为什么执行”；
- 账本不保存原文时，hash 必须覆盖协议、路径、请求体和规则版本，避免不同输入碰撞成同一证据；
- Dashboard、导出、CLI 和测试全部读取同一数据模型。

---

## 4. 产品级硬验收门槛

以下不是“以后再优化”的建议，而是产品对外宣称可用前必须满足的门槛。

### A. 语义正确性

- 支持协议 fixture 的工具 ID、名称、参数、消息顺序、事件顺序、finish reason、usage 和错误分类 100% 保真。
- 代码、配置、SQL、交易数据、签名、私钥和未知结构默认不改写。
- 规则无法证明安全、收益不为正、receipt 无法写入时，原样透传或明确阻断。

### B. 节省真实性

- 每一个节省数字都能对应原始 hash、发送 hash、规则版本和 usage 来源。
- Provider 有 measured usage 时，必须以 measured usage 做真实比较；无 measured usage 只标 estimated。
- 内部目标：工具密集任务的中位输入 Token 节省 ≥15%；没有收益的任务不因“必须优化”而被改写。
- 不把字节压缩直接宣传成美元节省；Prompt Cache、输出 Token 和重试必须单独核算。

### C. 性能与稳定性

- 在本地基准机上，≤1 MiB JSON 请求的规则处理 p95 <50ms；≤10 MiB 请求 p95 <200ms；超出基准必须透传或明确超限。
- 1,000 次连续请求和 100 并发请求无内部 panic、账本重复结算或资源泄漏。
- 流式空闲、客户端取消、Provider 错误、超限、进程重启后不残留不可解释的 `running`。

### D. 成本与控制

- 任意预算 scope 的并发预留不超过上限。
- 默认不重试未知副作用请求；Fallback 不转发前一 Provider 的凭证。
- 缓存、Fallback、路由和输出上限不能绕过预算、权限和循环保护。

### E. 用户体验

- 新用户三步内完成接入；不编辑配置文件也能运行最小路径。
- 用户 10 秒内看懂最近任务是否完成、花了多少、节省多少、为何被阻止。
- 任意优化都能一键绕过，且绕过后立即恢复原始请求路径。

### F. 隐私与恢复

- Prompt、代码、完整工具结果、Key、Cookie、私钥和签名默认不落盘。
- Profile 删除、导出、重启和强杀后的状态都有明确结果；不能用“完成”掩盖中断。

---

## 5. 开发执行顺序（不是分阶段降级，而是依赖顺序）

1. 先统一 `TaskRun / RequestAttempt / Receipt / Usage` 数据模型，避免后续 Dashboard 和账本返工。
2. 完成协议 IR 与语义护栏，再扩充规则；没有 IR 的规则不进入默认路径。
3. 完成工具结果 v2 与跨轮次去重，建立收益判断、回退和规则版本。
4. 补齐预算 scope、健康路由、熔断和重试解释；所有硬控制先于上游调用。
5. 完成缓存一致性、严格隐私模式和本地数据治理。
6. 完成 `doctor/install/launch/status/bypass/uninstall` 的零摩擦用户路径。
7. 重做 Dashboard 的任务视图、节省证明和行动指引。
8. 做基准、压力、协议 fixture、双跑和故障注入；最后才做包装、跨平台和外部兼容验收。

每个 Todo 合入前必须同时具备：代码、单元/fixture 测试、至少一条失败路径、receipt 字段、Dashboard/CLI 可解释信息，以及 bypass 回退路径。

---

## 6. 完成定义

当且仅当以下条件全部满足，才称为“产品力达到可对外推广”：

- 用户不用理解规则，也能稳定得到更短或原样的安全请求；
- 长任务的工具结果和历史上下文有可证明的节省，而不是只优化演示样本；
- Agent 任务结果、工具调用和流式协议不被破坏；
- 预算、重试、缓存和路由不会让用户悄悄多花钱；
- 每次节省、透传、阻断、回退和失败都能用人话解释；
- 本地数据、Key 和业务内容不离开用户环境；
- 出现任何不确定性时，系统选择保守透传，而不是自作聪明地改写；
- 用户可以随时绕过，并且绕过后仍能继续使用原来的 Agent。

这才是 DuoLA AgentCost 的产品价值：**不是“多一个代理层”，而是让 Agent 的每一次运行都更省、更稳、更可控，而且用户敢长期打开它。**

## 7. 本轮实际验收与后续增强边界

本轮已运行并通过：

- `cargo fmt --all`
- `cargo check --all-targets --all-features`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`：22 个测试通过
- `./tests/e2e.sh`：fake Provider、流式/取消、Fallback、预算 scope、缓存、严格隐私、规则注册表、旁路 API、控制事件和 JSON/CSV 导出通过
- `cargo build --release`、`cargo package --allow-dirty --no-verify`：通过

以下不是当前上线阻塞，而是后续增强或量化项：

1. **真实 Provider 双跑**：系统不会为了“证明节省”偷偷把有副作用的请求再发一遍；双跑用于后续量化节省效果，不是当前上线前提。
2. **完整跨协议 IR**：当前已覆盖已测的 OpenAI Chat/Responses、Anthropic 基础路径与通用 JSON 语义护栏；后续可继续把内部 Adapter 拆成统一 `RequestEnvelope/ResponseEvent`，不影响当前已验证路径。
3. **兼容矩阵扩展**：已测客户端和 Provider 只代表已验证路径；新增客户端、平台和安装包必须继续按回归清单扩展，不能据此宣称任意 Agent 均可用。

因此当前准确状态是：**本地核心能力已落地并通过自动化验收，已验证的 LinkAPI/Codex 路径可以试用；完整的普通用户上线仍被安装包、首次运行向导、真实客户端兼容矩阵和账户/收费闭环阻塞。**
