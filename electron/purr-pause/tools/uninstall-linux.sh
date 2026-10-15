#!/bin/bash
# 胖猫暂停一下（PurrPause） 完全清理脚本 (Linux)

echo "=== 胖猫暂停一下（PurrPause） 清理工具 (Linux) ==="
echo ""

# 卸载 deb 包
if dpkg -l purr-pause &>/dev/null; then
    echo "正在卸载 purr-pause..."
    sudo dpkg -r purr-pause
    echo "  已卸载"
else
    echo "  未检测到已安装的 deb 包"
fi

echo ""
echo "正在清理配置文件和数据..."

# 用户配置目录
CONFIG_DIR="$HOME/.config/purr-pause"
if [ -d "$CONFIG_DIR" ]; then
    rm -rf "$CONFIG_DIR"
    echo "  已删除: $CONFIG_DIR"
fi

# 辅助标记文件
MARK_FILE="$HOME/.local/share/.purr-pause-mark"
if [ -f "$MARK_FILE" ]; then
    rm -f "$MARK_FILE"
    echo "  已删除: $MARK_FILE"
fi

# 自启动文件
AUTOSTART="$HOME/.config/autostart/purr-pause.desktop"
if [ -f "$AUTOSTART" ]; then
    rm -f "$AUTOSTART"
    echo "  已删除: $AUTOSTART"
fi

echo ""
echo "清理完成！"
