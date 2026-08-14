#!/usr/bin/env bash
set -euo pipefail
grep -q 'model_provider = "duola-agentcost"' "$HOME/.codex/config.toml"
exit 0
