#!/bin/bash
# 胖猫暂停一下（PurrPause） 完全清理脚本 (macOS)

echo "=== 胖猫暂停一下（PurrPause） 清理工具 (macOS) ==="
echo ""

# 删除应用
APP_PATH="/Applications/purr-pause.app"
if [ -d "$APP_PATH" ]; then
    rm -rf "$APP_PATH"
    echo "  已删除: $APP_PATH"
else
    echo "  未检测到已安装的应用"
fi

echo ""
echo "正在清理配置文件和数据..."

# Application Support 目录
SUPPORT_DIR="$HOME/Library/Application Support/purr-pause"
if [ -d "$SUPPORT_DIR" ]; then
    rm -rf "$SUPPORT_DIR"
    echo "  已删除: $SUPPORT_DIR"
fi

# Preferences
PLIST="$HOME/Library/Preferences/com.purr-pause.app.plist"
if [ -f "$PLIST" ]; then
    rm -f "$PLIST"
    echo "  已删除: $PLIST"
fi

# Caches
CACHE_DIR="$HOME/Library/Caches/com.purr-pause.app"
if [ -d "$CACHE_DIR" ]; then
    rm -rf "$CACHE_DIR"
    echo "  已删除: $CACHE_DIR"
fi

# 辅助标记文件
MARK_FILE="$HOME/.local/share/.purr-pause-mark"
if [ -f "$MARK_FILE" ]; then
    rm -f "$MARK_FILE"
    echo "  已删除: $MARK_FILE"
fi

# Login Items (需要手动移除，脚本无法直接操作)
echo ""
echo "注意: 如果设置了开机自启，请手动在 系统设置 → 通用 → 登录项 中移除 purr-pause"

echo ""
echo "清理完成！"
