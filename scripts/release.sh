#!/usr/bin/env bash
# 出正式安装包:Intel 与 Apple Silicon 各一个 dmg,签名 + 公证 + 钉票,收进 安装包/
#
# 版本号唯一事实来源 = src-tauri/tauri.conf.json 的 version;
# Cargo.toml 与 package.json 必须与之一致,不一致直接报错(防止发错版本号)。
#
# 用法: ./scripts/release.sh
# 需要的环境变量(签名与公证):
#   APPLE_SIGNING_IDENTITY  证书名,不传则自动从钥匙串取 Developer ID Application
#   APPLE_ID / APPLE_APP_PASSWORD(或 APPLE_PASSWORD)  公证用
#   APPLE_TEAM_ID           不传则从证书里解析
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
CARGO_V=$(grep -m1 '^version' src-tauri/Cargo.toml | sed 's/.*"\(.*\)"/\1/')
PKG_V=$(python3 -c "import json;print(json.load(open('package.json'))['version'])")
if [ "$VERSION" != "$CARGO_V" ] || [ "$VERSION" != "$PKG_V" ]; then
  echo "✗ 版本号不一致: tauri.conf.json=$VERSION Cargo.toml=$CARGO_V package.json=$PKG_V"
  exit 1
fi
echo "版本 v$VERSION"

: "${APPLE_SIGNING_IDENTITY:=$(security find-identity -v -p codesigning 2>/dev/null \
    | grep 'Developer ID Application' | head -1 | sed 's/.*"\(.*\)"/\1/')}"
if [ -z "$APPLE_SIGNING_IDENTITY" ]; then
  echo "✗ 钥匙串里没有 Developer ID Application 证书,无法签名"; exit 1
fi
: "${APPLE_TEAM_ID:=$(echo "$APPLE_SIGNING_IDENTITY" | sed -n 's/.*(\([A-Z0-9]*\))$/\1/p')}"
: "${APPLE_PASSWORD:=${APPLE_APP_PASSWORD:-}}"
export APPLE_SIGNING_IDENTITY APPLE_TEAM_ID APPLE_PASSWORD
if [ -z "${APPLE_ID:-}" ] || [ -z "$APPLE_PASSWORD" ]; then
  echo "⚠ 未提供 APPLE_ID / APPLE_APP_PASSWORD:只签名不公证,用户打开会被 Gatekeeper 拦"
fi

OUT="安装包"
mkdir -p "$OUT"

# 分架构各出一个包(不用通用版:体积翻倍,且自行分发没有 App Store 的瘦身)
build() {
  local target=$1 label=$2
  echo ""
  echo "==== 构建 $label ($target) ===="
  npm run tauri build -- --target "$target" --bundles dmg
  local src
  src=$(find "src-tauri/target/$target/release/bundle/dmg" -name '*.dmg' | head -1)
  [ -n "$src" ] || { echo "✗ $label 没产出 dmg"; exit 1; }
  local dst="$OUT/DeskCat-$VERSION-$label.dmg"
  cp "$src" "$dst"
  echo "→ $dst ($(du -h "$dst" | cut -f1))"
}

build aarch64-apple-darwin apple-silicon
build x86_64-apple-darwin  intel

echo ""
echo "==== 验收 ===="
for dmg in "$OUT"/DeskCat-$VERSION-*.dmg; do
  mnt=$(mktemp -d)
  hdiutil attach -nobrowse -quiet "$dmg" -mountpoint "$mnt"
  app="$mnt/DeskCat.app"
  printf '%s\n  架构: %s\n  ' "$(basename "$dmg")" "$(lipo -archs "$app/Contents/MacOS/deskcat")"
  spctl -a -vv "$app" 2>&1 | head -1
  hdiutil detach "$mnt" -quiet
  rmdir "$mnt" 2>/dev/null || true
done
echo ""
echo "✅ 全部产出在 $OUT/"
