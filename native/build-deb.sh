#!/usr/bin/env bash
# 构建 PurrPause Linux 原生版 deb 包。
#
# 依赖（Debian/Ubuntu）：
#   sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
#        libmpv-dev libloading-dev librsvg2-bin desktop-file-utils
#
# 用法：
#   ./build-deb.sh              # 构建当前架构
#   ./build-deb.sh --debug      # debug 构建（不 strip，便于排查）

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

PROFILE="release"
if [[ "${1:-}" == "--debug" ]]; then
  PROFILE="debug"
fi

echo "==> 检查依赖"
need_pkg() {
  if ! pkg-config --exists "$1"; then
    echo "缺少依赖：$1" >&2
    echo "请先安装：sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libmpv-dev" >&2
    exit 1
  fi
}
need_pkg gtk4
need_pkg libadwaita-1

echo "==> cargo 构建（profile=${PROFILE}）"
cargo build --profile "$PROFILE"

BIN_PATH="target/$PROFILE/purr-pause-linux"
if [[ ! -x "$BIN_PATH" ]]; then
  echo "构建产物不存在：$BIN_PATH" >&2
  exit 1
fi

VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
ARCH="$(dpkg --print-architecture 2>/dev/null || echo amd64)"
PKG="purr-pause-linux_${VERSION}_${ARCH}"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "==> 装配 $PKG"
install -d "$STAGE/DEBIAN"
install -d "$STAGE/usr/bin"
install -d "$STAGE/usr/lib/$ARCH"
install -d "$STAGE/usr/share/purr-pause-linux/webm"
install -d "$STAGE/usr/share/purr-pause-linux/data"
install -d "$STAGE/usr/share/icons/hicolor/256x256/apps"
install -d "$STAGE/usr/share/icons/hicolor/scalable/apps"
install -d "$STAGE/usr/share/applications"

install -m 0755 "$BIN_PATH" "$STAGE/usr/bin/purr-pause-linux"

# 动态库：libmpv 是运行时依赖（大部分系统已装 mpv）
for lib in libmpv.so.2; do
  found="$(ldconfig -p | grep -oP "$lib\s*=>\s*\K[^ ]+" | head -1 || true)"
  if [[ -n "${found:-}" && -f "$found" ]]; then
    install -m 0755 "$found" "$STAGE/usr/lib/$ARCH/"
  fi
done

install -m 0644 data/webm/*.webm "$STAGE/usr/share/purr-pause-linux/webm/"
install -m 0644 data/config.json "$STAGE/usr/share/purr-pause-linux/config.json"
install -m 0644 data/README-assets.txt "$STAGE/usr/share/purr-pause-linux/data/"

install -m 0644 debian/purr-pause-linux.desktop "$STAGE/usr/share/applications/purr-pause-linux.desktop"

# 图标：优先 SVG，回退 PNG
if [[ -f data/icons/purr-pause-linux.svg ]]; then
  install -m 0644 data/icons/purr-pause-linux.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/purr-pause-linux.svg"
else
  # 用 ImageMagick 或 rsvg-convert 把 logo.png 转多尺寸 png
  if command -v convert >/dev/null 2>&1; then
    for sz in 16 24 48 128 256 512; do
      convert data/icons/logo.png -resize ${sz}x${sz} "$STAGE/usr/share/icons/hicolor/${sz}x${sz}/apps/purr-pause-linux.png" 2>/dev/null || true
    done
  fi
fi

install -m 0644 debian/control "$STAGE/DEBIAN/control"
install -m 0755 debian/postinst "$STAGE/DEBIAN/postinst"
install -m 0755 debian/prerm "$STAGE/DEBIAN/prerm"

sed -i "s|@ARCH@|$ARCH|g; s|@VERSION@|$VERSION|g" "$STAGE/DEBIAN/control"

# 生成 md5sums（放在 DEBIAN/ 下，且排除自身）
( cd "$STAGE" && find . -path ./DEBIAN -prune -o -type f -print0 \
  | xargs -0 md5sum | sed 's|  \./|  |' > DEBIAN/md5sums )

echo "==> 生成 deb"
mkdir -p dist

# 优先用 dpkg-deb（会处理权限与所有者）；缺失时退回纯 ar+tar 组装。
# 纯 ar 方案在 macOS / 无 dpkg 环境下也能工作，产物同样可被 dpkg 安装。
if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb --build --root-owner-group "$STAGE" "dist/${PKG}.deb"
else
  echo "    未找到 dpkg-deb，改用 ar+tar 组装"
  # 在临时目录里组装，成员名必须是 debian-binary / control.tar.gz / data.tar.gz。
  # ⚠️ 名字不能带前导点：ar 会把成员名原样存进归档，dpkg 只认无前缀的标准名，
  #    带点的名字（如 .build-control.tar.gz）会导致 dpkg 拒绝安装该包。
  AR_DIR="$(mktemp -d)"
  # control.tar.gz 必须包含 DEBIAN/ 目录前缀
  ( cd "$STAGE" && tar czf "$AR_DIR/control.tar.gz" DEBIAN )
  ( cd "$STAGE" && tar czf "$AR_DIR/data.tar.gz" --exclude=DEBIAN . )
  printf '2.0\n' > "$AR_DIR/debian-binary"
  rm -f "dist/${PKG}.deb"
  # ar 的成员顺序必须是 debian-binary, control.tar.gz, data.tar.gz
  ( cd "$AR_DIR" && ar rc "$SCRIPT_DIR/dist/${PKG}.deb" \
      debian-binary control.tar.gz data.tar.gz )
  rm -rf "$AR_DIR"
fi

SIZE="$(du -h "dist/${PKG}.deb" | cut -f1)"
echo ""
echo "构建完成：dist/${PKG}.deb ($SIZE)"
echo ""
echo "安装：sudo dpkg -i dist/${PKG}.deb"