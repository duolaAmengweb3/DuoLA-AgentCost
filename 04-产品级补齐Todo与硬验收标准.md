# DuoLA AgentCost：产品级补齐 Todo 与硬验收标准

> 目的：把“本地 Agent Gateway 能跑”提升为“用户可以稳定依赖、可以解释、可以安全回退的产品能力”。
>
> 本文只围绕产品能力：请求是否可靠地经过、规则是否不改变任务语义、成本和预算是否可控、失败是否可解释、异常是否能恢复。不把云端多租户、模型托管或营销指标冒充本地产品能力。
>
> 产品价值层面的主 Todo 见 [05-产品力升级Todo与硬验收标准.md](./05-产品力升级Todo与硬验收标准.md)；本文重点保留工程实现、真实环境和发布门禁，避免两份清单互相覆盖。

## 一、产品能力边界

### 已确定的产品承诺

1. Agent 的请求在用户本机经过 DuoLA AgentCost；Provider Key 仍由用户自己提供。
2. DuoLA 只做确定性、可解释的请求整理、工具结果压缩、精确缓存、路由、预算、限流、账本和恢复，不调用第二个模型替用户总结或改写任务。
3. 默认不保存 Prompt、代码和完整响应；账本只记录请求元数据、usage、费用和 hash receipt。
4. 任何优化都必须满足：规则执行成功、输出结构可解析、语义字段不被静默修改；否则原样透传或明确失败。
5. 用户可以随时 bypass；Gateway 异常时不能伪造模型完成结果。

### 不属于本次产品承诺

- 不承诺固定 Token 节省百分比；节省必须按真实 Provider usage 双跑测量。
- 不承诺所有 Agent、所有私有协议、所有 Provider 自动兼容；支持矩阵必须按协议和认证路径发布。
- 不承诺任意远程暴露安全；远程 Gateway 必须配置认证 Token，Admin 永远只允许回环地址。
- 不把语义缓存、自动输出改写、团队多租户、SSO 当成本地单用户版本已完成。

## 二、工程 Todo（产品级发布前）

### A. 请求边界与远程访问

- [x] Gateway 使用非回环地址时强制配置 `gateway_auth_token_env`。
- [x] 使用 `X-DuoLA-Gateway-Token` 验证请求；Token 不写入配置文件、不转发给 Provider。
- [x] Admin 继续强制绑定回环地址。
- [x] Gateway 使用 Axum `DefaultBodyLimit` 在读取 Body 前限制请求体。
- [x] 保留应用层 `max_request_bytes` 检查，给出明确错误。
- [ ] 真实部署验证：公网/局域网无 Token 启动失败；正确 Token 成功；错误 Token 返回 401；Provider 收不到 Gateway Token。

### B. 生命周期、取消与稳定性

- [x] 流式客户端断开时，取消上游消费并写入 `cancelled` / `cancelled_by_client`。
- [x] 流式正常结束、Provider 错误、响应超限、空闲超时都只能结算一次账本。
- [x] 流式和非流式响应都具备空闲超时；总超时仍由 HTTP Client 控制。
- [x] Gateway 收到 SIGINT/SIGTERM 后优雅停止监听，不伪造未完成响应。
- [ ] 真实终端验证：关闭 Codex/Claude 进程、断开 curl、杀掉上游连接，账本最终状态必须可解释且不留 `running`。
- [ ] 生产守护验证：launch、systemd/launchd、容器停止时都能完成恢复或明确 bypass。

### C. 预算与成本硬控制

- [x] Token 预算继续在上游调用前阻断。
- [x] Session/Daily Token 预算使用并发 reservation，避免并发请求同时通过预检查后共同超预算。
- [x] 配置 USD 预算时，按 Provider 价格和最大输出预算计算请求上限。
- [x] 没有显式输出上限时，AgentCost 自动写入协议对应的最大输出字段；无法识别 JSON 或输出字段时直接阻断，不放行无限成本请求。
- [x] 并发请求使用 USD reservation，避免多个请求同时通过预检查后共同超预算。
- [x] Fallback 使用同协议 Provider 的最坏价格做预留，实际结算按最终 Provider usage。
- [ ] 真实 Provider 验收：设置极小 request/session/daily USD 限额，分别验证输入超限、输出超限和并发超限都在上游调用前阻断。
- [ ] 双跑验收：账本的 measured usage、估算 usage、cached usage 和真实账单可对账。

### D. 缓存与内存

- [x] 精确缓存默认关闭。
- [x] 缓存拒绝流式、工具、执行标记、随机采样和状态型请求。
- [x] 缓存按条数、单条大小和总字节数三重限制。
- [x] 读取和写入时清理过期项；LRU 淘汰同步减少总字节数。
- [x] 缓存命中仍先经过循环、预算和并发策略，不得绕过安全门禁。
- [ ] 压测验收：开启缓存后，在配置的总字节上限内连续写入大响应，RSS 不随请求数无限增长。

### E. Fallback 与请求语义

- [x] 同协议 Provider 才允许进入 Fallback 链。
- [x] 已开始向客户端输出后不再切换 Provider。
- [x] GET/HEAD/OPTIONS 默认可重试；带 `Idempotency-Key` 的请求可重试。
- [x] OpenAI Responses/Chat、Anthropic Messages 等模型生成协议保留 POST Fallback；未知协议的非幂等请求默认不重试。
- [x] 未知协议可以通过 `routing.allow_non_idempotent_fallback=true` 显式承担重试风险。
- [ ] 真实外部 API 验收：一个有副作用的 POST 在默认配置下不得被重复发送；显式开启后必须在账本中记录策略。

### F. 配置、Profile 与恢复

- [x] `--config` 对应独立 data 目录，ledger、bypass、Codex snapshot 不与默认 Profile 混用。
- [x] Provider、预算、缓存和路由可通过 Admin reload 热加载。
- [x] 监听地址、认证方式和 Body 上限变更要求重启，CLI 和 Dashboard 必须明确提示。
- [x] 配置目录 0700、配置文件/账本辅助文件 0600（Unix）；Windows 使用平台权限语义。
- [ ] 多 Profile 验收：两个配置同时运行，Provider、ledger、bypass、Codex snapshot 互不影响。
- [ ] Codex 配置被用户手动改动时，恢复必须拒绝覆盖并给出明确处理方式。

### G. 兼容性与正式交付

- [x] fake Provider 覆盖 OpenAI Responses、Chat 兼容路径、Anthropic SSE、长 SSE、Fallback、缓存和预算。
- [x] 真实 LinkAPI OpenAI-compatible Endpoint：Chat Completions 工具结果、20 个工具定义、Responses 流式、真实 Codex CLI、Token 预算拦截和 `response.failed` 分类。
- [ ] 原生 OpenAI 官方 Endpoint：非流式、流式、工具调用、usage、429/5xx。
- [ ] 真实 Anthropic Messages：非流式、流式、usage、message_error。
- [ ] 真实 Codex：只读任务、配置接管、退出恢复、Gateway 崩溃恢复。
- [ ] 真实 Claude Code：Anthropic base URL 接入、工具调用、流式中断。
- [ ] Cursor BYOK：确认实际请求是否经过 Gateway；不把 Cursor 私有订阅流量写成已支持。
- [ ] OpenCode/其他 OpenAI-compatible：至少完成一组协议差异验证。
- [ ] macOS arm64/x86_64、Linux x86_64/arm64、Windows x86_64 构建矩阵。
- [ ] 每个发布包提供版本、SHA-256、兼容矩阵、变更记录和回滚说明。
- [ ] 依赖安全检查：`cargo audit` 或 `cargo deny` 纳入发布 CI。

## 三、硬验收标准

### 1. 正确性

- 任何请求经过 Gateway 后，Provider 看到的业务字段与规则预期一致。
- 未命中规则时原样透传；规则无法安全执行时不得静默改写。
- 流式响应的终止事件、失败事件、usage 结算和 HTTP 状态一致。
- 每个请求最多有一次最终账本结算，不能同时出现 completed 和 cancelled。

### 2. 预算

- Token 预算在上游发送前阻断。
- 并发请求的 Session/Daily Token 预留不得超过配置上限。
- USD 预算配置后，不允许在未知最大输出的情况下无限放行。
- 并发请求的累计预留不得超过 session/daily USD 上限。
- Fallback 按最坏价格预留，实际账本按最终 Provider 结算。

### 3. 资源

- 请求体不超过配置上限；超过时在上游调用前返回 413。
- 非流式和流式响应不超过配置上限。
- 单条缓存、缓存总量、过期清理和 LRU 淘汰均可从状态接口观察。
- 流式空闲超过配置值必须释放上游连接和并发槽位。

### 4. 安全

- 非回环 Gateway 没有 Token 时启动失败。
- 错误 Token 返回 401；Token 不出现在 Provider 请求、receipt、ledger、日志和 Dashboard。
- Admin 永不绑定非回环地址。
- 默认不保存 Prompt、代码、完整响应和 API Key。

### 5. 可恢复性

- bypass 是一条可验证的原样转发路径。
- Gateway 正常收到停止信号时优雅退出。
- 强制中止后，下一次启动会把遗留 running 标记为 interrupted，而不是 completed。
- Codex 配置被用户修改时，恢复操作必须拒绝覆盖。

### 6. 可观测性

- Dashboard/API 能看到最终 Provider、每次 attempt、失败原因、usage、费用、节省字节和规则 receipt。
- blocked、rate_limited、loop_blocked、budget_blocked、cancelled、timeout 均有稳定状态值。
- Ledger 写入失败不能伪造“已完成证据”；至少输出可检索的本地错误。

### 7. 发布门禁

以下是面向普通用户公开发布前的硬门禁；本地核心测试通过不等于这些门禁已经通过：

1. 原生 OpenAI/Anthropic 各一组非流式和 SSE 流式会话（真实 LinkAPI OpenAI-compatible 验证已完成）；
2. 真实 Codex、Claude Code 各一组只读任务和退出恢复；
3. 至少一个 Cursor BYOK 请求确认实际走 Gateway；
4. 原始路径与 Gateway 路径双跑，对比任务结果、Provider usage、延迟和账单；
5. macOS、Linux、Windows 至少各有一个可安装产物；
6. 远程 Gateway 认证、取消、预算、请求体、缓存和恢复的负向测试全部通过。

## 四、本轮实施后的状态定义

- **已完成：** 本地 Gateway 核心闭环、自动化 fake Provider 验收、已验证的 LinkAPI/Codex 路径、远程监听认证、资源上限、取消记账、空闲超时、缓存总量、Token/USD 预算预留、Fallback 语义保护、配置 Profile 隔离、优雅退出。
- **仍阻塞公开发布：** 原生 Provider/Claude/Cursor 兼容性、跨平台安装包、首次运行向导、真实双跑数据、账户/收费闭环和官网到本机 Dashboard 的完整入口。
- **仍不做虚假承诺：** 固定 Token 节省比例、超出已测矩阵的未知 Agent、远程多租户、语义缓存、自动输出改写。
