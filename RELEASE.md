# DuoLA AgentCost 发布清单

## 已完成的本地闭环

- Rust 单二进制构建：`cargo build --release`
- 本地 Gateway：127.0.0.1，OpenAI Responses / Chat Completions / Anthropic Messages 透明转发
- 流式响应透传，不等待完整响应才返回
- BYOK：Key 只从用户环境变量读取
- Provider 列表、Fallback、价格配置和实际 usage 账本
- 工具结果/工具面确定性规则与 receipt（ANSI、重复长行、紧凑 JSON、同请求去重、大工具面筛选）
- 静态规则注册表、工具面副作用保守停裁、语义护栏和结构化 Anthropic tool result 处理
- 显式安全精确缓存（TTL/LRU/清空；工具/流式/执行请求排除）
- priority/cost 路由、同协议 Fallback、显式成本池、Token/美元预算、output cap、限流、并发和重复请求暂停
- 请求/响应硬上限；长 SSE 终止事件尾部扫描；缓存键按凭证/租户相关头隔离，缓存命中仍经过预算和循环门禁
- 请求体由框架和应用双重限制；流式/非流式空闲超时；客户端断开写入 cancelled；Gateway 支持 SIGINT/SIGTERM 优雅退出
- Gateway 非回环监听强制环境变量 Token；未知协议非幂等 Fallback 默认关闭；带 `Idempotency-Key` 才允许安全重试
- Token/硬 USD 预算并发预留，按同协议 Provider 最坏价格计算；缓存同时有条数、单条字节、总字节和过期清理
- 所有配置命令和 `launch` 支持自定义配置路径；每个 Profile 独立 ledger/bypass/Codex snapshot；Provider 可 reload；Admin 强制回环绑定；Gateway 重启会收敛 running 账本为 interrupted
- Codex 配置快照、冲突检测和退出恢复
- Dashboard、status、stats、doctor、export、uninstall
- Dashboard 任务/趋势视图、一键旁路/恢复、`/api/rules`、`/api/cache/status`、`/api/control-events`
- 敏感 Provider 错误脱敏、JSON/CSV 脱敏账本导出、事务化本地数据清理
- fake Provider 端到端：转发、规则、fallback、循环暂停、bypass、账本、Codex 恢复
- fake Provider 端到端：缓存命中、输出上限、Token 预算和 Dashboard 统计
- fake Provider 端到端：超过 2 MiB 后才到达终止事件的长 SSE，以及缓存不能绕过 Token 预算

## 当前发布状态

当前版本是 **本地核心 release candidate**，不是已经完成普通用户全平台上线的 SaaS。公开官网提供真实安装入口和 macOS 固定包；DuoLA 账户、云端账本和模型托管不属于当前版本。

已完成的真实路径：

- macOS Apple Silicon/Intel 预编译二进制、SHA-256 校验和；
- `duola-agentcost setup` 自动检测 Agent、准备最小 Provider；
- `duola-agentcost launch codex --open-dashboard` 自动启动 Gateway、接入 Agent、打开本机 Dashboard；
- 公开官网根路径、安装脚本、Dashboard 预览和固定下载包已分开。

仍不能宣称已完成：

- Linux/Windows 真实安装包在目标机器上的验收；
- 原生 OpenAI/Anthropic 账号、Claude Code、Cursor 的完整兼容矩阵；
- DuoLA Pro/Team 账户、计费和云端用量关联。

## 真实用户验收记录

以下真实场景已由产品负责人完成测试并确认通过；后续新增 Provider、客户端或平台时继续作为回归清单。它们不能替代跨平台安装包和普通用户首次运行门禁。

这些步骤需要真实的第三方账号或客户端，不能用本地假服务替代：

1. 用用户自己的 OpenAI API 或兼容 Endpoint 发一次非流式请求。
2. 发一次 SSE 流式请求，确认首个事件不会被等待，最终 usage 能落账。
3. 用 Anthropic API 发一次 Messages 请求，确认 `x-api-key` 和 `anthropic-version` 等原始头按用户配置透传。
4. 用真实 Codex 或 Claude Code 执行一个只读任务，确认退出后原配置仍可恢复。
5. 人为停止 Gateway，确认用户可执行 `bypass` 回到原始 Provider。
6. 远程 Gateway 绑定 `0.0.0.0`：未配置 Token 必须启动失败；正确 `X-DuoLA-Gateway-Token` 成功；错误 Token 返回 401。
7. 中途关闭流式客户端，确认 ledger 状态为 `cancelled`，Provider attempt 为 `cancelled_by_client`，不存在遗留 `running`。
8. 设置极小 USD budget，验证输出上限和并发预留在上游请求前阻断。
9. 使用两个自定义 `--config` 同时运行，确认账本、bypass 和 Codex snapshot 完全隔离。

真实验证应记录：客户端版本、协议、模型、Provider、是否流式、账本 JSON、请求 ID 和结果，不上传 Prompt、代码和 API Key。

## 发布命令

```bash
make verify
cargo package --allow-dirty
```

`make verify` 是本地自动化门禁；真实 Provider/Agent、远程认证、跨平台产物仍必须按 [产品级补齐 Todo 与硬验收标准](./04-产品级补齐Todo与硬验收标准.md) 执行。

发行包只包含源代码和锁文件；不把 `target/`、本地数据库、配置、Provider Key 或测试临时目录带入发布包。

## 不对外承诺

- 固定 Token 节省百分比；
- 拦截 ChatGPT/Claude 私有订阅后端；
- 所有 Agent 和所有客户端内置流量；
- 自动修改用户代码；
- 把 Prompt、代码、日志上传到 DuoLA；
- “有 fallback 就一定重试成功”。已开始返回流后绝不重放请求。
- 语义缓存、模型回答自动改写或固定压缩比例；这些能力没有默认开启，避免让相似请求或输出改写破坏任务语义。
