#!/usr/bin/env bash
# 一键打包 PurrPause 的 Linux 安装包（.deb / AppImage，x64 / arm64）。
#
# 用法：
#   ./scripts/build-deb.sh                 # 打 deb（当前主机架构，= 原有行为）
#   ./scripts/build-deb.sh --arm64         # 打 arm64 的 deb（x64 主机可交叉打，无需 ARM 机器）
#   ./scripts/build-deb.sh --x64 --arm64   # x64 + arm64 两个 deb
#   ./scripts/build-deb.sh --appimage      # 同时产出 AppImage（与 deb 一起）
#   ./scripts/build-deb.sh --all           # deb + AppImage，x64 + arm64（共 4 个产物）
#   ./scripts/build-deb.sh --clean         # 打包前清掉 dist/ 下旧的 .deb / .AppImage
#   ./scripts/build-deb.sh --publish never # 其余未识别参数原样透传给 electron-builder
#   （以上 flag 可组合，例如：./scripts/build-deb.sh --appimage --arm64 --clean）
#
# 环境变量：
#   ELECTRON_MIRROR   指定 Electron 下载镜像（国内加速），例如：
#                     ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/ ./scripts/build-deb.sh
#
# 说明：
#   - 底层调用 electron-builder（npm run build -- --linux <目标> <架构>）。
#   - 本项目无原生依赖，故可在 x64 主机交叉打 arm64：electron-builder 会自动下载对应
#     架构的 Electron 预编译再重打包。AppImage 所需 appimagetool 由 electron-builder
#     构建时自动下载，无需额外 apt 包。
#   - 额外做了：定位项目根目录、检查 node/npm/binutils、缺依赖时自动 npm install、
#     可选清空 dist/、结束后列出全部产物与大小。
set -euo pipefail

# 始终切到项目根目录（脚本可从任意位置调用）
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

log() { echo "胖猫暂停一下（PurrPause） $*"; }
die() { echo "✗ $*" >&2; exit 1; }

# 解析参数：消费已知 flag，其余原样透传给 electron-builder
CLEAN=0
WANT_APPIMAGE=0
WANT_ALL=0
ARCHES=()
PASSTHRU=()
for arg in "$@"; do
  case "$arg" in
    --clean)    CLEAN=1 ;;
    --appimage) WANT_APPIMAGE=1 ;;
    --all)      WANT_ALL=1 ;;
    --x64)      ARCHES+=("--x64") ;;
    --arm64)    ARCHES+=("--arm64") ;;
    *)          PASSTHRU+=("$arg") ;;
  esac
done

# 目标格式与架构
TARGETS=(deb)
if [ "$WANT_APPIMAGE" -eq 1 ] || [ "$WANT_ALL" -eq 1 ]; then
  TARGETS=(deb AppImage)
fi
if [ "$WANT_ALL" -eq 1 ]; then
  ARCHES=(--x64 --arm64)   # --all 覆盖为双架构
fi
# 架构都不给时不传 --x64/--arm64 → electron-builder 用当前主机架构（保持原有默认）

# 1) 平台提示
if [ "$(uname -s)" != "Linux" ]; then
  log "提示：当前系统不是 Linux，electron-builder 在非 Linux 上打 Linux 包可能需要 Docker。"
fi

# 2) Node / npm 检查
command -v node >/dev/null 2>&1 || die "未找到 node，请先安装 Node.js（建议 18+）。"
command -v npm  >/dev/null 2>&1 || die "未找到 npm，请先安装 Node.js / npm。"
NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
if [ "$NODE_MAJOR" -lt 18 ]; then
  log "提示：检测到 Node $(node -v)，建议使用 18 及以上版本。"
fi

# 3) 系统打包工具：electron-builder 用 fpm/strip 生成 deb 与 AppImage，需要 binutils（ar/strip）
if [ "$(uname -s)" = "Linux" ] && ! command -v ar >/dev/null 2>&1; then
  die "缺少 ar（binutils），打包会报 \"Need executable 'ar'\"。请先安装：sudo apt-get install -y binutils"
fi

# 4) 依赖未安装时自动安装（首次会下载 Electron 二进制，需联网）
if [ ! -d node_modules ]; then
  if [ -n "${ELECTRON_MIRROR:-}" ]; then
    log "未检测到 node_modules，安装依赖（使用 Electron 镜像 ${ELECTRON_MIRROR}）..."
  else
    log "未检测到 node_modules，安装依赖（首次会下载 Electron，需联网）..."
    log "  如下载慢，可中断后用：ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/ ./scripts/build-deb.sh"
  fi
  npm install
fi

VERSION="$(node -p "require('./package.json').version")"

# 5) 可选清理旧产物
if [ "$CLEAN" -eq 1 ]; then
  log "清理旧的 dist/*.deb / dist/*.AppImage ..."
  rm -f dist/*.deb dist/*.AppImage
fi

# 6) 组装 electron-builder 参数并打包
EB_ARGS=(--linux "${TARGETS[@]}")
if [ "${#ARCHES[@]}" -gt 0 ]; then EB_ARGS+=("${ARCHES[@]}"); fi
if [ "${#PASSTHRU[@]}" -gt 0 ]; then EB_ARGS+=("${PASSTHRU[@]}"); fi

log "开始打包（版本 ${VERSION}）：目标=[${TARGETS[*]}] 架构=[${ARCHES[*]:-host}]"
npm run build -- "${EB_ARGS[@]}"

# 7) 校验并列出产物
shopt -s nullglob
ARTIFACTS=(dist/*.deb dist/*.AppImage)
shopt -u nullglob
if [ "${#ARTIFACTS[@]}" -eq 0 ]; then
  die "打包似乎已结束，但未在 dist/ 找到 .deb / .AppImage，请检查上方 electron-builder 输出。"
fi

log "✓ 打包完成，产物（dist/）："
for f in "${ARTIFACTS[@]}"; do
  log "    ${f}  （$(du -h "$f" | cut -f1)）"
done
for f in dist/*.deb; do
  [ -e "$f" ] || continue
  log "  deb 安装测试：sudo dpkg -i \"${ROOT_DIR}/${f}\""
  break
done
for f in dist/*.AppImage; do
  [ -e "$f" ] || continue
  log "  AppImage 运行：chmod +x \"${ROOT_DIR}/${f}\" && \"${ROOT_DIR}/${f}\""
  break
done
