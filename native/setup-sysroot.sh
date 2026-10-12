#!/usr/bin/env bash
# 在 macOS 上准备 Linux 交叉编译 sysroot（Ubuntu 24.04 noble）。
#
# 用途：让 cargo 能从 macOS 交叉编译出 Linux 二进制。
# 原理：下载 Ubuntu 的 -dev 包（含头文件 + .so 符号链接 + pkg-config 文件），
#      解压到一个目录，通过 PKG_CONFIG_SYSROOT_DIR / PKG_CONFIG_PATH 指向它。
#
# 用法：
#   ./setup-sysroot.sh              # 下载并解压到 target/sysroot
#   ./setup-sysroot.sh --clean# 清理
#
# 交叉编译还需要 Linux 链接器：
#   brew install zig       # 或装一个 x86_64-linux-gnu 的 gcc 工具链
#   然后：
#     export PKG_CONFIG_SYSROOT_DIR=$PWD/target/sysroot
#     export PKG_CONFIG_PATH=$PWD/target/sysroot/usr/lib/x86_64-linux-gnu/pkgconfig
#     rustup target add x86_64-unknown-linux-gnu
#     cargo build --target x86_64-unknown-linux-gnu
#
# 注意：GTK4 依赖链很深，缺符号时链接会失败。
#      若遇到问题，直接用 CI（.github/workflows/build-native.yml）更省事。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

SYSROOT="$SCRIPT_DIR/target/sysroot"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [[ "${1:-}" == "--clean" ]]; then
  rm -rf "$SYSROOT" "$SCRIPT_DIR/target/.cross"
  echo "已清理 sysroot"
  exit 0
fi

MIRROR="${APT_MIRROR:-http://archive.ubuntu.com/ubuntu}"
DIST="${DIST:-noble}"
ARCH="${ARCH:-amd64}"

echo "==> 下载 Ubuntu ${DIST} (${ARCH}) 包索引"
curl -sS -m 180 -o "$WORK/Packages.gz" \
  "${MIRROR}/dists/${DIST}/main/binary-${ARCH}/Packages.gz"
gunzip -c "$WORK/Packages.gz" > "$WORK/Packages"

echo "==> 解析需要的包"
python3 - "$WORK/Packages" "$WORK/urls.txt" <<'PY'
import re, sys
pkgs_file, out_file = sys.argv[1], sys.argv[2]
want = {
    # GTK4 与 libadwaita
    'libgtk-4-dev', 'libgtk-4-1', 'libadwaita-1-dev', 'libadwaita-1-0',
    'libgraphene-1.0-dev', 'libgraphene-1.0-0', 'libepoxy-dev', 'libepoxy0',
    # GLib 栈
    'libglib2.0-dev', 'libglib2.0-0t64', 'libcairo2-dev', 'libcairo2',
    'libpango1.0-dev', 'libpango-1.0-0', 'libgdk-pixbuf-2.0-dev', 'libgdk-pixbuf-2.0-0',
    'libharfbuzz-dev', 'libharfbuzz0b', 'libfribidi-dev', 'libfribidi0',
    'libxkbcommon-dev', 'libxkbcommon0', 'zlib1g-dev', 'zlib1g',
    # 渲染/字体后端
    'libfontconfig1-dev', 'libfreetype6-dev',
}
txt = open(pkgs_file, encoding='utf-8', errors='ignore').read()
found = {}
for b in txt.split('\n\n'):
    m = re.search(r'^Package: (\S+)$', b, re.M)
    if m and m.group(1) in want:
        f = re.search(r'^Filename: (\S+)$', b, re.M)
        if f:
            found[m.group(1)] = f.group(1)
missing = want - set(found)
with open(out_file, 'w') as fh:
    for _, path in sorted(found.items()):
        fh.write(path + '\n')
print(f"  找到 {len(found)}/{len(want)} 个包")
if missing:
    print(f"  缺失（可能不影响编译）: {', '.join(sorted(missing))}")
PY

echo "==> 下载并解压"
mkdir -p "$SYSROOT"
count=0
while read -r rel; do
  count=$((count + 1))
  url="${MIRROR}/${rel}"
  deb="$WORK/pkg.deb"
  if ! curl -sS -m 180 -o "$deb" "$url"; then
    echo "  [$count] 下载失败，跳过: $(basename "$rel")"
    continue
  fi
  # deb 内部数据可能是 zst / xz / gz
  for member in data.tar.zst data.tar.xz data.tar.gz; do
    if ar t "$deb" 2>/dev/null | grep -q "^${member}$"; then
      case "$member" in
        *.zst) ar p "$deb" "$member" 2>/dev/null | tar --use-compress-program=unzstd -xf - -C "$SYSROOT" 2>/dev/null || true ;;
        *.xz)  ar p "$deb" "$member" 2>/dev/null | tar -xJf - -C "$SYSROOT" 2>/dev/null || true ;;
        *.gz)  ar p "$deb" "$member" 2>/dev/null | tar -xzf - -C "$SYSROOT" 2>/dev/null || true ;;
      esac
      break
    fi
  done
  printf "  [%d] %s\n" "$count" "$(basename "$rel")"
done < "$WORK/urls.txt"

echo ""
echo "==> sysroot 就绪：$SYSROOT"
echo ""
echo "关键 pkg-config 文件："
find "$SYSROOT" -name "*.pc" 2>/dev/null | grep -E "(gtk4|libadwaita|glib-2.0|cairo|pango)\.pc" | sed "s|$SYSROOT||" | head -8
echo ""
echo "下一步："
echo "  export PKG_CONFIG_SYSROOT_DIR=$SYSROOT"
echo "  export PKG_CONFIG_PATH=$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig"
echo "  rustup target add x86_64-unknown-linux-gnu"
echo "  cargo build --release --target x86_64-unknown-linux-gnu"