$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw "未找到 Rust/Cargo。当前源码安装器需要 Cargo；正式发布请使用对应平台的预编译安装包。"
}

cargo install --path $Root --locked --force
duola-agentcost setup --non-interactive
Write-Host "DuoLA AgentCost 已安装。直接执行：duola-agentcost launch codex --open-dashboard"
