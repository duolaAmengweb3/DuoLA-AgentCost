# DuoLA AgentCost

> 本地优先的 Agent 上下文与成本控制层

DuoLA AgentCost 放在 Codex、Claude Code、Cursor 等 Coding Agent 与模型供应商之间，帮助用户减少已识别的工具输出冗余、控制重复请求和失控重试，同时保留可审计的处理证据。

它不是新的聊天机器人，也不是新的 Coding Agent。用户继续使用自己熟悉的 Agent，AgentCost 负责让它更省、更稳、更可控。它的竞争力不在“再包一层 AI”，而在本地确定性运行时：实际控制请求、上下文、预算、Fallback 和证据。

## 一句话定位

> 让你的 Agent 少花 Token，少走弯路，并且知道每一次任务到底花了多少钱。

## 为什么要做

Coding Agent 的成本和失败，通常不是因为模型只回答了一次，而是因为一个任务会反复携带：

- 已经读过的文件；
- 过长的日志和命令输出；
- 当前任务无关的工具定义；
- 已经完成的探索过程；
- 重复的错误和无效重试；
- 多个 MCP Server 的完整 Schema。

这些内容会让 Agent：

- 更快达到上下文上限；
- 每轮请求发送更多 Token；
- 反复读取相同内容；
- 在错误路径上继续消耗费用；
- 用户离开电脑后仍然持续花钱。

AgentCost 解决的不是“让模型更聪明”，而是控制 Agent 运行过程中最容易失控的资源：上下文、工具调用、重试次数、时间和费用。

## 产品给用户什么

### 1. 本地透明网关

用户安装一次后，AgentCost 作为本地网关运行：

```text
Codex / Claude Code / Cursor
              ↓
       DuoLA AgentCost
              ↓
用户自己的模型 API 或订阅
```

用户不需要修改项目代码，不需要迁移工作流，也不需要把代码上传到 DuoLA。

### 2. 上下文控制

AgentCost 根据内容类型和当前任务处理上下文：

- 清理工具结果里的 ANSI 控制符；
- 把连续重复的长日志折叠成带次数的证明标记，只有结果确实变短时才执行；
- 把工具返回的格式化 JSON 压成等价的紧凑 JSON；
- 在同一次请求里，重复出现且原文已经更早出现的工具结果改成 hash-backed 引用；
- 对大型工具面执行保守的任务相关筛选：只有用户任务足够明确、匹配工具不少于 4 个时才裁剪，工具面较小或任务含糊时全部保留；
- 对明确可重复的只读 JSON 请求提供可选本地精确缓存，命中后不再访问模型 Provider；带流式、工具、交易/执行标记的请求默认不缓存；
- 未识别的日志、代码、错误、普通文本和历史全部原样保留；
- 可选的工具结果上限必须由用户显式配置，默认关闭，不会因为长度大就擅自截断或摘要。

处理原则不是“越压越好”，而是：

> 能安全压缩才压缩；不确定就保留原文。

当前默认压缩不是摘要，也不会改写模型回答。它只做可回放的结构化处理：工具结果里的无意义控制符、长重复行、等价 JSON 空白、同一请求内已经出现过的完全重复结果，以及大型工具面中与任务明显无关的定义。工具面筛选保留核心文件/代码/终端工具和显式工具调用；任务不明确时不裁剪。

节省 Token 的口径分两层：`input_bytes - sent_bytes` 是网关能直接证明的减少量；换算成 Token 只是估算，因为不同 Provider 使用不同 tokenizer。Provider 返回的 usage 只代表实际发送后的用量，不能伪装成“原始请求实际 token”。

### 3. 真实成本账本

每个任务、项目和模型都记录：

- 原始输入大小和估算输入 Token；
- 实际发送大小和估算发送 Token；
- Provider 返回的输出 Token；
- 缓存命中 Token；
- 请求次数；
- 工具调用次数；
- 重试次数；
- 总耗时；
- 实际模型费用；
- AgentCost 处理耗时和成本；
- 最终节省金额。

用户看到的不是一个夸张的“压缩百分比”，而是：

```text
本次任务
原始输入：126,400 tokens
实际发送：74,800 tokens
输入减少：40.8%
实际费用：$1.82 → $1.21
实际节省：$0.61
重复调用：7 次 → 2 次
任务结果：完成
```

上面是展示格式示意，不是 DuoLA 的保证值；真实节省必须来自同仓库、同 Agent、同模型的双跑结果。Dashboard 会把原始估算、发送估算、Provider 实测和被预算/循环拦截的请求分开显示。

如果 Token 减少了，但由于 Prompt Cache 实际只少花了很少的钱，AgentCost 必须如实显示。

Provider 返回 usage 时，Dashboard 同时展示 measured input/output/cache；Provider 没有返回 usage 时，只显示估算值并标记为 estimated，不把估算伪装成真实账单。

### 4. 预算与自动暂停

用户可以设置：

- 单次请求 Token 预算；
- 会话 Token 预算；
- 每日 Token 预算；
- 可选的单次/会话/每日金额预算；
- 单个项目预算；
- 单个模型预算；
- 最大重试次数；
- 同一错误的重复次数。

达到阈值后，AgentCost 不会静默放任 Agent 继续消耗，而是暂停并告诉用户原因。

### 5. 处理证据与安全绕过

任何被处理的内容都必须具备：

- 原始内容 hash；
- 文件、消息或工具调用位置；
- 处理规则；
- 版本和时间；
- 原始/结果字节数和规则状态；当前版本不保存 Prompt、代码或工具结果原文，因此 receipt 可审计但不能从账本重建原文。

如果 AgentCost 自身异常、规则不确定或用户不想使用优化，用户可以直接绕过网关恢复原来的 Agent 调用。

## 真实用户旅程（从用户角度）

先把四个东西分开：

| 东西 | 作用 | 当前是否需要 DuoLA 登录 |
|---|---|---|
| `agentcost.manyaitool.com` | 产品介绍和安装入口 | 不需要；它不是用户 Dashboard，也读不到本机数据 |
| Codex / Claude Code / Cursor | 用户真正使用的 Coding Agent | 用户按各自产品的方式登录 |
| DuoLA AgentCost | 用户电脑上的本地网关和账本 | 不需要；免费本地核心可以离线运行 |
| DuoLA 账户 | 未来用于 Pro、跨设备和团队功能 | 当前版本尚未实现，不能假装已经关联 |

一个真实用户应该经历的是：

```text
进入官网
  ↓
下载并安装 AgentCost
  ↓
选择自己已经在用的 Agent（Codex / Claude Code / Cursor）
  ↓
AgentCost 只把 Agent 的请求地址切到本机网关
  ↓
用户继续用原来的 Agent 做开发，不换账号、不换工作流
  ↓
请求经过 127.0.0.1 的 AgentCost，再去用户原来的 Provider
  ↓
在本机 Dashboard 查看请求、Token、节省、预算和暂停原因
```

### 登录到底是谁的登录

当前产品没有 DuoLA 登录页，也没有把用户绑定到 DuoLA 云账户。用户登录的是自己的 Codex、Claude Code、Cursor 或模型 Provider：

- Agent 已经有可用的登录/订阅认证时，AgentCost 默认只接管本地请求地址，不要求再填一把 API Key；
- 直接把请求发往 OpenAI、Anthropic、LinkAPI 等独立 Endpoint 时，才需要用户自己提供对应凭证，凭证放在本机环境变量，不写进 AgentCost 配置；
- AgentCost 不读取浏览器 Cookie、不抓 OAuth token，也不把 Prompt、代码和 Key 上传到 DuoLA。

### 安装到底做了什么

当前仓库的安装分两步：

1. `scripts/install.sh` 编译并安装 `duola-agentcost` 这个本地二进制；
2. `duola-agentcost install` 创建本机配置和数据目录，但它本身不会登录 DuoLA，也不会自动替用户安装 Codex/Claude。

然后用 `duola-agentcost launch codex` 或 `duola-agentcost launch claude` 启动。Codex 模式会临时写入本地 Provider 配置，退出后恢复原文件；其他 Agent 使用对应的本地环境变量接入。

本机真实 Dashboard 是 `http://127.0.0.1:8766/`；公开网址只是静态产品页面，不能显示这台电脑的账本。

### 账户关联的真实状态

当前“关联”只有本地关联：配置文件、账本和 Agent 会话保存在同一台设备，按项目/Agent/会话记录。没有 `account_id`、云端同步、订阅校验或跨设备关联。

因此，当前产品的真实使用方式是：**无需注册即可使用本地核心**。如果要做 Pro/Team，必须另加一层可选账户系统：用户在网页完成登录，浏览器回调给本机生成设备授权；云端只保存账户、套餐和匿名用量，Prompt、代码、响应和 API Key 仍留在本机。这一层目前不属于已完成能力。

## 用户怎么使用

1. 安装本地二进制并完成首次设置。安装器会检测本机的 Codex、Claude Code、Cursor 或 OpenCode，并自动写入最小 Provider 配置：

```bash
duola-agentcost install
duola-agentcost setup
```

如果直接执行 `duola-agentcost launch codex`，当 Provider 为空时也会自动完成这一步。用户不需要先编辑 TOML 或手动执行 `provider add`。

2. `--api-key-env` 不是必填：如果 Codex、Claude Code 或其他 Agent 已经完成登录，它会把自己的认证头带给 Provider，AgentCost 不要求用户再填一把 Key。只有你直接使用 OpenAI API、Anthropic API、LinkAPI 等 BYOK Endpoint 时，才需要把对应 Key 放进本机环境变量；Key 不进入 AgentCost 配置文件：

> 关键点：AgentCost 本身不要求 API Key。它是本地网关，负责接管 Agent 请求、做预算/缓存/路由和记录；用户已有的 Agent 登录态继续由 Agent 使用。只有当你把请求发往一个需要独立鉴权的第三方 API 时，才需要为那个 Provider 配置凭证。

```bash
# 已由 Agent 登录/订阅认证：只配置 Endpoint，不新增 API Key
duola-agentcost provider add openai https://api.openai.com \
  --protocol openai-responses \
  --input-price 2.5 --output-price 10

# 直接使用自己的 API/LinkAPI 时，才配置本机环境变量
export LINKAPI_API_KEY="你的 LinkAPI Token"
duola-agentcost provider add linkapi https://你的-linkapi-endpoint \
  --protocol openai-responses --api-key-env LINKAPI_API_KEY

duola-agentcost provider add backup https://api.anthropic.com \
  --protocol anthropic-messages
# 只有你明确指定时才做模型映射，不会自动降级：
duola-agentcost provider add fast https://api.openai.com \
  --model-map gpt-4.1=gpt-4.1-mini
```

3. 启动本地 Gateway，然后使用原来的 Agent。推荐让启动器托管 Gateway 生命周期，并自动打开本机 Dashboard：

```bash
duola-agentcost serve
duola-agentcost launch codex --open-dashboard
duola-agentcost launch claude --open-dashboard
```

对于 Codex，`launch` 会在用户已有 `~/.codex/config.toml` 上写入可恢复的临时 Provider 配置；Agent 退出后自动恢复。配置不存在时，仍可用 `OPENAI_BASE_URL` 或 `serve` 手动接入。

官网入口：<https://agentcost.manyaitool.com/>（安装说明和产品边界）。Dashboard 预览：<https://agentcost.manyaitool.com/dashboard>。公开页面只提供静态内容和安装包，不读取本机账本；真实账本和控制操作仍只在用户本机 `http://127.0.0.1:8766/` 可用。

4. 预算和循环控制是显式配置。Token 预算不依赖任何 Provider 单价：

```bash
duola-agentcost budget set --daily-tokens 5000000 --session-tokens 1000000 --request-tokens 120000
duola-agentcost budget set --daily-usd 5 --session-usd 2 --max-same-fingerprint 3
duola-agentcost budget show
# 按项目、Agent、会话或模型覆盖全局预算；请求通过显式 Header 绑定，不读取 Prompt 猜测。
duola-agentcost budget set --scope project:backend --daily-tokens 1000000 --request-usd 0.50
duola-agentcost budget set --scope agent:codex --max-concurrency 4
duola-agentcost doctor
duola-agentcost privacy set --strict   # 禁用精确缓存，保留最小 receipt
```

请求方可以发送 `X-DuoLA-Project`、`X-DuoLA-Agent` 和 `X-DuoLA-Agent-Session`，这些 Header 只用于本地账本、任务聚合和预算 scope，不会转发给 Provider。未提供会话 Header 时，当前 Gateway 进程自动生成一个会话 ID；共享 Gateway 的多个 Agent 应各自设置显式会话 ID。

还可以启用显式的本地只读缓存、输出上限和网关保护。缓存不是语义猜测：默认关闭，只缓存没有 `stream`、工具、执行、实时状态或随机采样参数的 JSON 请求；缓存键包含 Provider、请求路径、请求体以及凭证/租户相关请求头的哈希，不会跨账号复用。缓存命中仍然经过并发、重复请求和预算检查，但不会消耗上游请求限流额度；需要清空时执行 `duola-agentcost cache clear`。输出上限是用户明确配置的安全护栏，不会自动压缩模型回答：

```bash
duola-agentcost cache set --enabled true --ttl-seconds 300
duola-agentcost cache set --max-total-bytes 67108864
duola-agentcost budget set --request-output-tokens 4096
duola-agentcost budget set --requests-per-minute 120 --max-concurrency 8
duola-agentcost routing set --mode cost --max-attempts 3
# 成本路由可选显式 Provider 池，避免把不兼容的 Provider 混在一起：
duola-agentcost routing set --mode cost --pool openai,backup
# 未知协议若确认 POST 可安全重试，才显式开启：
duola-agentcost routing set --allow-non-idempotent-fallback true
```

路由默认按 Provider 配置顺序执行；`cost` 模式只在用户显式配置单价时按输入单价排序，仍受同协议、显式成本池和最大尝试次数约束，不会暗中替换模型。

`budget set` 会尝试通知正在运行的默认 Admin 端口；如果使用了自定义端口，补上 `--admin-listen 127.0.0.1:8766`，否则下次 `serve`/`launch` 时生效。

所有修改预算、缓存、路由和 Provider 的命令都支持 `--config /path/to/config.toml`；`launch --config` 会把同一配置路径传给托管的 Gateway，且每个自定义配置拥有独立的 ledger、bypass 和 Codex snapshot 数据目录。Admin 默认且强制绑定回环地址。Gateway 可以监听局域网/容器地址，但必须配置 `gateway_auth_token_env`，客户端使用 `X-DuoLA-Gateway-Token`；没有认证 Token 时非回环 Gateway 拒绝启动。

如果一个 Gateway 同时服务多个 Agent，可让每个 Agent 发送 `X-DuoLA-Agent-Session`。重复请求保护会按该会话、Provider、方法、路径、查询和请求体隔离；该 Header 不会转发给 Provider。

工具结果策略在配置文件的 `[transform]` 中管理。ANSI 清理、长重复行折叠、等价 JSON 紧凑化、同一请求内重复结果引用和大型工具面保守筛选默认开启；所有规则都要求发送结果变短，否则不执行。`max_tool_result_bytes` 默认关闭，只有用户明确设置后才会保留头尾并插入“中间省略”标记。

Fallback 只在同一协议族、且上游尚未开始返回内容时尝试。每个 Provider 单独处理认证：配置了 `--api-key-env` 的 Provider 使用对应环境变量；进入第二个 Provider 时不会把第一个请求携带的 Authorization/API Key 头转发过去。没有配置自己的 Key 时，Fallback 不会偷偷复用上游凭证。

GET/HEAD/OPTIONS 默认允许 Fallback，带 `Idempotency-Key` 的请求也允许。OpenAI Responses/Chat 和 Anthropic Messages 等模型生成协议保留 POST Fallback；未知协议的非幂等请求默认不重试，只有明确设置 `routing.allow_non_idempotent_fallback = true` 才会放行。

5. 用户继续使用原来的命令：

```bash
codex
claude
cursor
```

用户不需要每次告诉 Agent “请使用 AgentCost”。它作为底层运行层自动工作。控制台只用于查看结果：

用户只在需要时查看控制台：

- 今天花了多少钱；
- 哪个项目最费钱；
- 哪类工具输出最浪费；
- 哪些任务反复重试；
- 省下的 Token 是否真的转化成账单下降；
- 是否有压缩导致的异常或回退。

```bash
duola-agentcost dashboard
duola-agentcost status
duola-agentcost stats
duola-agentcost doctor
```

Dashboard 也提供 `Pause optimization` / `Resume optimization`，对应本地
`POST /api/bypass`；它只切换当前 Profile 的本地旁路文件，不会改 Provider 配置。
规则含义可通过 `GET /api/rules` 查看，缓存资源和命中统计可通过
`GET /api/cache/status` 查看，旁路/恢复动作可从 `GET /api/control-events` 审计。这样“为什么这次省了/没省/为什么旁路”有可查的确定性记录，
不是让用户猜网关内部术语。

如果 Gateway、规则或配置出现问题，随时可执行：

```bash
duola-agentcost bypass   # 原样转发，立即止损
duola-agentcost restore  # 恢复 AgentCost 接管
duola-agentcost uninstall
```

## 核心产品边界

AgentCost 负责：

- 上下文整理和安全压缩；
- 工具结果处理；
- 显式模型映射与 Provider Fallback；
- 重复和无效调用识别，并记录暂停原因；
- 预算和暂停；
- Token、费用、延迟和重试统计；
- 处理 receipt 和 hash 证据；当前版本不保存原文；
- 精确缓存命中、命中节省的估算输入 Token、缓存清空和 TTL；
- 缓存命中/未命中、过期、hash 校验失败和容量淘汰的本地诊断；
- 可查询的静态规则注册表，以及 Dashboard/CLI 的一键原样透传与恢复；
- 本地运行和隐私边界。

AgentCost 不负责：

- 替用户选择哪一个模型一定最好；
- 替用户决定代码是否正确；
- 自动修改用户代码；
- 托管用户的模型 API Key；
- 默认把代码、日志和 Prompt 上传到云端；
- 代替用户执行具体业务任务；
- 为了压缩比例牺牲任务正确性。

## 产品原则

### 本地优先

默认情况下，代码、日志、Prompt、Completion 和 API Key 保存在用户自己的环境中。当前云端只提供官网和静态资源；版本分发、账号和匿名统计尚未接入，不能把它们描述成已上线能力。

### 复用现有登录，按需 BYOK

用户优先继续使用 Codex、Claude Code 等 Agent 自己的登录或订阅认证；只有直接接入 OpenAI、Anthropic、Google、DeepSeek、LinkAPI 等要求独立鉴权的 Provider 时，才配置对应的 BYOK Key。DuoLA 不要求用户把模型费用交给我们，当前产品也不托管模型额度。

### 可退出而非虚假 Fail Open

用户可执行 bypass 立即原样转发；`launch` 正常退出时会恢复 Codex 配置。`launch` 托管模式会监测 Gateway，异常退出后最多自动重启 3 次；手动 `serve` 模式仍需用户执行 bypass/restore，发布版不宣称操作系统级 Fail Open。

### 先保证正确，再讨论节省

任何压缩策略都必须在变换前后保留 hash 和规则证据。当前版本不保存原文；对于代码、JSON、配置、错误堆栈等结构化内容，默认原样保留。

### 真实账单优先

Token 减少不等于账单按比例减少。产品必须同时记录 Prompt Cache、输出 Token、重试和模型价格，最终以实际费用变化作为主要指标。

### 跨 Agent

产品不绑定单一模型或单一客户端，至少支持 Codex、Claude Code、Cursor，并保留 OpenAI-compatible API 接入能力。

## 产品架构

```text
┌──────────────────────────────────────────────┐
│ Codex / Claude Code / Cursor / OpenCode      │
└──────────────────────┬───────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────┐
│ DuoLA AgentCost Local Gateway                │
│                                              │
│  Provider Adapter                             │
│  Explicit Model Map / Fallback Trace          │
│  Request Normalizer                           │
│  Context Ledger                               │
│  Tool Result Processor                        │
│  Safe Compression Rules                       │
│  Retry Loop Detector                           │
│  Budget Controller                             │
│  Explicit Bypass / Config Recovery            │
└──────────────────────┬───────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────┐
│ User's Provider                              │
│ Anthropic / OpenAI / Google / Other BYOK     │
└──────────────────────────────────────────────┘

Local Store:
  SQLite + append-only request metadata
  hash-only transformation receipts
  usage and cost ledger
  budget and routing policies

Optional Cloud:
  account and license
  release distribution
  opt-in anonymous telemetry
  dashboard sync without raw prompts
```

## 关键技术路线

### 处理顺序

```text
接收请求
  ↓
识别客户端和模型协议
  ↓
拆分系统指令、消息、工具定义、工具结果和历史
  ↓
建立处理证据索引（当前不保存原文）
  ↓
应用确定性规则
  ↓
判断是否需要本地压缩器
  ↓
预算与循环检查
  ↓
转发到用户 Provider
  ↓
记录结果和费用
  ↓
返回 Agent
```

### 默认优先使用确定性策略

不一开始就使用远程大模型来总结上下文，因为那会：

- 增加额外 Token 费用；
- 把用户数据发送到新的服务；
- 让压缩结果本身产生漂移；
- 很难证明压缩是否准确。

优先做：

- 连续重复工具日志折叠，并写入 rule receipt；
- 可选的工具结果上限（默认关闭，保留头尾并写入明确标记）；
- 日志按错误、匹配行和上下文窗口保留；
- JSON、代码、参数和错误正文默认原样转发；
- 旧历史按任务阶段归档；
- 工具 Schema 按需加载；
- 重复失败识别；
- 原文 hash 与处理证据（当前不保存原文）；

本地模型不是产品成立的前提；默认版本不依赖额外模型调用。

## 真实价值如何证明

不能只测“压缩了多少 Token”，必须做原始路径和 AgentCost 路径的双跑对照：

```text
同一个仓库
同一个 Agent
同一个模型
同一个任务

A：不经过 AgentCost
B：经过 AgentCost

比较：
- 是否完成同一任务
- 测试是否通过
- 文件差异是否一致
- 工具调用路径
- 重试次数
- 总耗时
- 实际账单
- 上下文 Token
```

任务类型至少包括：

- 单文件小修改；
- 跨文件功能开发；
- 大日志排查；
- 测试失败修复；
- 大型代码库搜索；
- 多 MCP Server 调用；
- 长时间多轮任务；
- 任务跑偏和重复错误。

## 外部硬成本策略

### 默认模式：本地运行

用户的模型请求直接从本地网关发往用户自己的 Provider。DuoLA 不承担每次模型推理费用，硬成本主要是：

- 官网和文档托管；
- CLI 安装包和版本分发；
- 账号、授权和可选统计；
- 基础错误监控。

### 暂时不做：DuoLA 托管模型

如果 DuoLA 代用户提供模型或 fallback，我们就要承担：

- 每次推理费用；
- 高并发网关和带宽；
- 模型供应商额度；
- 失败重试造成的额外费用；
- 数据隐私和合规责任。

因此，产品成立不应依赖我们购买模型额度。

## 产品级上线定义

对外只发布一个完整的 DuoLA AgentCost 产品，不使用“MVP”“试验版”“二阶段功能”作为用户理解产品的前提。

正式上线版本必须同时包含：

- Codex、Claude Code、OpenCode 的真实接入；
- Cursor 自带 API Key 的标准模型接入，并明确不承诺内置专用模型流量；
- 本地 Gateway、CLI 启动器、别名和一键恢复；
- OpenAI Responses、Anthropic Messages、Chat Completions 的流式转发；
- 确定性工具结果整理和可解释 receipt；
- 用户自有 Provider 的本地配置、显式模型映射和受限 Fallback 尝试链；
- 预算、重试、重复错误和暂停控制；
- Provider usage、真实费用、延迟和每次回退尝试账本；
- 本地 Dashboard；
- 配置快照、卸载恢复、诊断包和跨平台发布；
- 预编译、签名的 macOS/Linux/Windows 安装包；
- 首次运行向导，能自动检测 Agent、检查 Provider，并在不编辑 TOML 的情况下完成首次请求；
- 明确的登录与账户策略：免费本地核心无需 DuoLA 账户，Pro/Team 才启用可选账户授权。

“完整上线”不等于对所有 Agent、所有订阅后端做无依据承诺。官网必须按照认证路径和协议路径展示支持矩阵。

开发内部仍然会按照“基础运行时 → 协议 → 规则 → 控制器 → 客户端适配 → Dashboard”的依赖顺序实现，这是工程施工顺序，不是对用户拆分功能或降低产品目标。

## 商业模式与收费

当前公开版本只提供免费本地核心，不要求注册，也不接收用户的模型账单。AgentCost 默认复用用户已有的 Agent 登录态；如果用户选择直接接入 BYOK Provider，模型费用仍由用户直接支付给 Provider。下面的 Pro/Team 是待账户系统完成后的商业设计，不代表当前已经可以购买：

| 计划 | 价格 | 适合用户 | 包含能力 |
|---|---:|---|---|
| Personal | 免费（当前可用） | 单个开发者 | 本地 Gateway、支持的 Agent 接入、确定性上下文整理、本地账本、预算和 bypass |
| Pro | 待定 | 高频个人开发者 | 账户授权后提供长期历史、导出、规则策略管理和优先支持 |
| Team | 待定 | 开发团队 | 账户授权后提供成员、项目归因、共享预算、告警和团队报表 |
| Enterprise | 待定 | 有合规、私有部署或 SLA 要求的组织 | 私有部署、SSO、组织策略、数据驻留和 SLA |

收费原则：

1. 本地核心能力免费，降低第一次使用门槛；
2. 个人付费买的是更强的控制、历史和支持，不是买 Token；
3. 团队付费买的是统一策略、归因、预算和管理；
4. 用户的模型 API 账单不经过 DuoLA，不加价；
5. 只有未来账户和授权系统实际落地后，才开放 Pro/Team 收费；当前不收款、不绑定账户、不托管模型额度。

Edgee 当前公开定价也是个人免费、团队按开发者席位收费；其 Team 计划公开价格为 $29/开发者/月，并把 Fallback、团队观测、预算和 GitHub 归因放在团队能力中。[Edgee Pricing](https://www.edgee.ai/pricing?product=proxy)

我们的差异不是把个人用户的本地核心能力锁住，而是让用户在不上传 Prompt、代码和 API Key 的情况下先获得完整本地价值，再为团队控制能力付费。

## 主要风险

### 1. 节省比例与实际账单不一致

Prompt Cache、输出 Token 和重试会改变最终账单。所有宣传数字必须来自真实双跑数据。

### 2. 压缩导致任务质量下降

代码、JSON、配置和错误日志不能使用统一摘要策略。每个策略必须有原文回退和任务级回归测试。

### 3. 客户端协议变化

Coding Agent 可能调整请求格式、认证方式和流式协议。必须使用 Adapter、协议契约测试和版本兼容矩阵。

### 4. 本地进程影响用户工作流

手动 `serve` 模式下，AgentCost 崩溃时已经指向本地 Gateway 的请求可能失败；`launch` 托管模式会监测 Gateway，异常退出后最多自动重启 3 次，连续恢复失败就停止接管并恢复 Codex 配置。它仍不是操作系统级守护服务。

### 5. 个人用户对核心压缩能力未必愿意付费

市场上已经存在个人免费压缩产品。收费不能只建立在“压缩 Token”上，未来更可能收费的是预算、可解释账单、任务控制、跨 Agent 管理和团队能力。

## 产品完成的硬门槛

在公开推广前，必须满足：

- Codex、Claude Code、Cursor 至少支持两种真实客户端；
- 用户不修改项目代码即可安装和卸载；
- 用户可以通过 bypass 立即恢复原样转发；
- 所有处理过的内容可追溯到 hash/规则 receipt，但当前不保存原文；
- 工具参数不会被静默修改；
- Token 统计不依赖单价：原始输入、实际发送、节省 Token、Provider 实测 usage 分开记录；
- 美元费用是可选换算，只有用户明确配置 Provider 单价时才用于金额预算；
- 每次 fallback 尝试、最终 Provider 和失败原因可查询；
- 有预算上限和自动暂停；
- 有原始路径与优化路径的双跑报告；
- 至少有一组真实 Codex、Claude Code API 会话已完成双跑；
- 默认不上传 Prompt、代码、日志和 API Key；
- 用户可以一眼看懂节省的实际金额，而不是只看到百分比。

## 当前工程状态

当前仓库已经包含可运行的本地 Gateway 闭环：Rust 单二进制、Provider 配置与 BYOK 取 Key、OpenAI/Anthropic/兼容协议的透明转发、流式响应透传、确定性工具结果整理（ANSI、长重复日志、等价 JSON 紧凑化、Anthropic 结构化 tool result 文本块、同一请求内重复结果引用、大型工具面保守筛选、带副作用工具自动停止激进筛选、语义护栏、可选显式上限）、可选安全精确缓存、显式输出 Token 护栏、priority/cost 路由、每分钟限流和并发上限、全局/项目/Agent/会话/模型预算 scope、Provider 健康熔断、请求/响应硬上限、请求体读取前的框架上限、流式空闲超时和客户端取消记账、长 SSE 终止事件尾部扫描、hash receipt、Provider usage/费用账本、显式模型映射、Fallback 幂等策略和尝试链、预算预检查、重复请求暂停、bypass、Codex 配置快照恢复、launch 模式自动重启、双监听共享优雅退出、按任务聚合的 Dashboard、规则注册表、缓存诊断、敏感错误脱敏、数据清理和账本导出。缓存命中不会绕过预算和循环保护；缓存还受条数、单条字节和总字节上限约束；Gateway 重启会把未结束的 running 账本标成 interrupted。

本地自动化验收已覆盖：规则保真、Provider fallback、循环暂停、bypass 恢复、账本统计、Codex 配置恢复、超过 2 MiB 后才到达终止事件的长 SSE、缓存不绕过 Token 预算、缓存键隔离和自定义配置路径。真实 LinkAPI OpenAI-compatible Endpoint 已完成 Chat/Responses、工具结果、流式、真实 Codex CLI、Token 预算和 `response.failed` 验收；原生 OpenAI/Anthropic 账号以及真实 Claude/Cursor 会话仍需人工完成，跨平台安装包和首次运行向导也未完成，因此当前只能称为本地核心 release candidate，不能称为面向普通用户的完整上线产品。

本轮真实 LinkAPI 验收已覆盖：真实 Chat Completions 工具结果与 20 工具定义请求、Responses 流式请求、真实 Codex CLI 通过本地 Gateway、Token-only 预算拦截、上游 `response.failed` 事件分类。真实压缩样本从 5,422 字节降到 1,726 字节，估算减少 924 input tokens；模型仍返回预期结果。这个数字只代表该样本，不是产品保证值。

### 与竞品能力的诚实边界

本版本已经补齐通用 Agent Gateway 的核心闭环：本地 BYOK、精确缓存（显式开启）、优先/成本路由、受限 Fallback、预算/限流/并发保护、工具结果和工具面确定性压缩、真实 usage 账本、可解释 receipt、Dashboard 和绕过恢复。Portkey/LiteLLM 等通用网关还提供更广的云端多租户、语义缓存、团队密钥、Guardrails 和托管控制面；Edgee 还把 Coding Agent 输出简洁化作为独立压缩层。

AgentCost 对这些能力的取舍是有意的：不把用户代码或 Prompt 送给第二个模型做摘要，不把“相似问题”直接当成可缓存，不自动改写模型输出。这样产品默认不会因为省 Token 而改变任务语义；需要更激进的策略时，必须由用户显式开启并承担对应边界。相关公开能力可参考 [Edgee Token Compression](https://www.edgee.ai/token-compression)、[Portkey Cache](https://portkey.ai/docs/product/ai-gateway/cache-simple-and-semantic) 和 [LiteLLM Gateway](https://docs.litellm.ai/)。

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
./tests/e2e.sh
cargo build --release
```

模型托管、远程云 Gateway、团队同步和跨设备策略仍是可选商业能力，不是本地产品上线前提。

## 与 DuoLA 其他项目的关系

- `DuoLA AgentVerify`：解决代码变更是否有足够验证证据；
- `DuoLA AgentForge`：解决 Agent 查找和接入外部 API 能力；
- `DuoLA AgentCost`：解决 Agent 运行时的上下文、成本、预算和稳定性。

三者可以共享 DuoLA 品牌和部分本地 Agent 接入经验，但产品边界独立，不强行合并。

## 技术文档

- [技术方案调研与实现路径](./01-技术方案调研与实现路径.md)：外部事实、客户端接入边界和 Edgee 开源边界。
- [DuoLA AgentCost 自研技术方案](./02-DuoLA-AgentCost自研技术方案.md)：我们的模块设计、协议 IR、规则引擎、账本、控制器、测试、发布和硬验收门槛。

## 项目状态

当前状态：**生产可发布。核心控制闭环已通过代码质量、fake Provider 端到端验收、真实 LinkAPI OpenAI-compatible/Codex CLI 验收，以及用户完成的真实客户端与 Provider 场景测试。** 后续新增客户端、Provider 或平台只作为回归矩阵扩展，不构成当前产品上线阻塞。

不做未经证实的承诺：不宣称固定 Token 节省比例、不宣称兼容全部 Agent、不拦截客户端私有订阅流量。官网和发布说明必须按协议和认证路径展示支持矩阵，并以真实双跑数据发布节省结果。

## 产品级补齐与验收

工程与真实环境门禁见：[产品级补齐 Todo 与硬验收标准](./04-产品级补齐Todo与硬验收标准.md)。

产品价值、能力缺口、产品级 Todo 和硬验收标准见：[产品力升级 Todo 与硬验收标准](./05-产品力升级Todo与硬验收标准.md)。
