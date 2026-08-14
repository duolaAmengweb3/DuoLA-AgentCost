#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${DUOLA_AGENTCOST_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)}"
TARGET="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
OUT="$ROOT/dist"
NAME="duola-agentcost-v${VERSION}-${TARGET}"

mkdir -p "$OUT"
cargo build --release --locked --target "$TARGET"
BIN="$ROOT/target/$TARGET/release/duola-agentcost"
[ -x "$BIN" ] || { echo "未找到构建产物：$BIN" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/$NAME"
cp "$BIN" "$TMP/$NAME/duola-agentcost"
cp "$ROOT/README.md" "$ROOT/RELEASE.md" "$TMP/$NAME/"
tar -C "$TMP" -czf "$OUT/$NAME.tar.gz" "$NAME"
(cd "$OUT" && shasum -a 256 "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
echo "已生成：$OUT/$NAME.tar.gz"
echo "校验：$OUT/$NAME.tar.gz.sha256"
