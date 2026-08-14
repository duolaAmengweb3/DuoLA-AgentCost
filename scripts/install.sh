#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cargo install --path "$ROOT" --locked --force
duola-agentcost setup --non-interactive
printf '\nDuoLA AgentCost 已安装。直接执行：duola-agentcost launch codex --open-dashboard\n'
printf '如果你使用 Claude Code：duola-agentcost launch claude --open-dashboard\n'
