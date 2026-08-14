#!/usr/bin/env bash
set -euo pipefail

# Public installer: downloads a fixed release artifact. For local development
# from a checkout, use the repository's scripts/install.sh instead.
VERSION="${DUOLA_AGENTCOST_VERSION:-0.1.4}"
BASE_URL="${DUOLA_AGENTCOST_RELEASE_BASE_URL:-https://github.com/duolaAmengweb3/DuoLA-AgentCost/releases/download/v${VERSION}}"
OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS/$ARCH" in
  Darwin/arm64) TARGET="aarch64-apple-darwin" ;;
  Darwin/x86_64) TARGET="x86_64-apple-darwin" ;;
  Linux/x86_64) TARGET="x86_64-unknown-linux-gnu" ;;
  *) echo "暂不支持的平台：$OS/$ARCH。请从 Releases 下载对应安装包。" >&2; exit 1 ;;
esac
NAME="duola-agentcost-v${VERSION}-${TARGET}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl --fail --location --silent --show-error "$BASE_URL/${NAME}.tar.gz" -o "$TMP/${NAME}.tar.gz"
curl --fail --location --silent --show-error "$BASE_URL/${NAME}.tar.gz.sha256" -o "$TMP/${NAME}.tar.gz.sha256"
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$TMP" && sha256sum -c "${NAME}.tar.gz.sha256")
else
  (cd "$TMP" && shasum -a 256 -c "${NAME}.tar.gz.sha256")
fi
tar -xzf "$TMP/${NAME}.tar.gz" -C "$TMP"
DEST="${DUOLA_AGENTCOST_INSTALL_DIR:-$HOME/.local/bin/duola-agentcost}"
mkdir -p "$(dirname "$DEST")"
install -m 0755 "$TMP/${NAME}/duola-agentcost" "$DEST"
"$DEST" setup --non-interactive
echo "安装完成：duola-agentcost launch codex --open-dashboard"
case ":${PATH}:" in
  *":$(dirname "$DEST"):"*) ;;
  *) echo "如果终端找不到命令，先执行：export PATH=\"$(dirname "$DEST"):\$PATH\"" ;;
esac
