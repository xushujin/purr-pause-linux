# 胖猫暂停一下（PurrPause） 部署与运维文档

## 1. 系统要求

### 1.1 硬件要求

| 项目 | 最低要求 | 推荐配置 |
|------|---------|---------|
| CPU | 双核 x86_64 | 四核及以上 |
| 内存 | 512 MB 可用 | 1 GB 可用 |
| 磁盘 | 200 MB | 500 MB |
| 显卡 | 支持 GPU 加速 | 独立显卡或集成显卡均可 |

### 1.2 软件要求

| 项目 | 要求 |
|------|------|
| 操作系统 | Ubuntu 20.04+ / Debian 11+ |
| 桌面环境 | GNOME、KDE、XFCE 等（需 X11 或 XWayland） |
| 空闲检测 | xprintidle（推荐）或 GNOME Mutter |

### 1.3 安装空闲检测工具

```bash
# 推荐方式（适用于所有桌面环境）
sudo apt install xprintidle

# GNOME 桌面环境自带 gdbus，无需额外安装
```

## 2. 安装

### 2.1 通过 .deb 包安装

```bash
# 下载最新版本
wget https://github.com/purr-pause/purr-pause/releases/latest/download/purr-pause_1.4.0_amd64.deb

# 安装
sudo dpkg -i purr-pause_1.4.0_amd64.deb

# 如有依赖问题
sudo apt-get install -f
```

使用 `sudo dpkg -i` 安装时，安装后脚本会先修正 `/opt/purr-pause/chrome-sandbox` 的所有权和权限（`root:root`、`4755`），避免 Electron 启动时报 SUID sandbox 权限错误；随后为执行 sudo 的桌面用户创建 `~/.config/autostart/purr-pause.desktop`，下次登录桌面后自动启动。

### 2.2 验证安装

```bash
# 检查是否安装成功
dpkg -l | grep purr-pause

# 启动应用
/opt/purr-pause/purr-pause
```

## 3. 卸载

```bash
# 卸载应用
sudo dpkg -r purr-pause

# 清除包级配置（用户数据仍需手动删除）
sudo dpkg --purge purr-pause

# 手动清理用户配置（可选）
rm -rf ~/.config/purr-pause/
```

## 4. 配置文件

### 4.1 配置文件位置

| 文件 | 路径 | 说明 |
|------|------|------|
| 用户配置 | `~/.config/purr-pause/config.json` | 用户自定义设置（优先读取） |
| 默认配置 | 应用内置 `config.json`（打包在 app.asar 中） | 应用内置默认值 |

### 4.2 配置文件格式

```json
{
  "thresholdMinutes": 45,
  "breakMinutes": 5,
  "animationMode": "walk-center",
  "customWebmDir": "",
  "walkVideo": "cat-walk.webm",
  "idleVideo": "cat-rest.webm"
}
```

### 4.3 配置项说明

| 字段 | 说明 | 默认值 | 有效范围 |
|------|------|--------|---------|
| `thresholdMinutes` | 连续使用多久后提醒（分钟） | 45 | 1 ~ 480 |
| `breakMinutes` | 休息倒计时时长（分钟） | 5 | 1 ~ 60 |
| `animationMode` | 动画模式（仅激活用户可切换） | `walk-center` | `walk-zoom` / `fade-center` / `walk-flat` / `walk-center` |
| `customWebmDir` | 自定义素材目录（仅激活用户可设置） | `""` | 有效目录路径 |
| `walkVideo` | 走路动画文件名 | `cat-walk.webm` | 有效文件名 |
| `idleVideo` | 休息动画文件名 | `cat-rest.webm` | 有效文件名 |

### 4.4 手动编辑配置

```bash
# 创建配置目录（首次运行后会自动创建）
mkdir -p ~/.config/purr-pause

# 编辑配置
nano ~/.config/purr-pause/config.json
```

手动编辑配置文件后需要重启应用才会重新读取；通过设置界面修改会立即生效。

## 5. 自定义视频素材

### 5.1 素材目录

```
~/.config/purr-pause/webm/
```

首次运行后，应用会自动创建此目录并生成 `purr-pause-素材说明.txt` 说明文件。

### 5.2 素材要求

| 属性 | 要求 |
|------|------|
| 格式 | WebM 容器，VP9 编码 |
| Alpha 通道 | 必须有（实现透明背景） |
| 建议尺寸 | 200~500px 宽高 |
| 背景 | 必须透明 |

### 5.3 替换步骤

```bash
# 1. 准备你的 WebM 视频文件（带 Alpha 通道）
# 2. 复制到素材目录
cp my-cat-walk.webm ~/.config/purr-pause/webm/cat-walk.webm
cp my-cat-rest.webm ~/.config/purr-pause/webm/cat-rest.webm

# 3. 如果使用非默认文件名，编辑配置文件
nano ~/.config/purr-pause/config.json
# 添加 "walkVideo": "my-walk.webm", "idleVideo": "my-idle.webm"
```

### 5.4 使用 FFmpeg 制作透明视频

```bash
# 从带 Alpha 通道的 PNG 序列生成 WebM
ffmpeg -framerate 24 -i frame_%04d.png \
  -c:v libvpx-vp9 -pix_fmt yuva420p \
  -b:v 1M -auto-alt-ref 0 \
  output.webm
```

## 6. 自启动配置

### 6.1 使用设置页开关

右键托盘图标 → 设置 → 勾选“开机自动启动”→ 保存。Ubuntu / GNOME / KDE 等桌面环境下，应用会写入当前用户的 `~/.config/autostart/purr-pause.desktop`；取消勾选会删除该文件。

### 6.2 手动创建 .desktop 文件

```bash
# 创建自启动条目
mkdir -p ~/.config/autostart

cat > ~/.config/autostart/purr-pause.desktop << EOF
[Desktop Entry]
Type=Application
Name=胖猫暂停一下（PurrPause）
Comment=胖猫暂停一下 - 屏幕休息提醒
Exec=/opt/purr-pause/purr-pause
Icon=purr-pause
Terminal=false
Hidden=false
NoDisplay=false
X-GNOME-Autostart-enabled=true
Categories=Utility;
EOF
```

### 6.3 使用 systemd 用户服务

```bash
mkdir -p ~/.config/systemd/user

cat > ~/.config/systemd/user/purr-pause.service << EOF
[Unit]
Description=胖猫暂停一下（PurrPause） - 屏幕休息提醒
After=graphical-session.target

[Service]
Type=simple
ExecStart=/opt/purr-pause/purr-pause
Restart=on-failure
RestartSec=5
Environment=DISPLAY=:0

[Install]
WantedBy=default.target
EOF

# 启用并启动
systemctl --user enable purr-pause
systemctl --user start purr-pause
```

## 7. 故障排除

### 7.1 常见问题

#### 猫咪不出现

1. **检查空闲检测工具是否可用**：
   ```bash
   xprintidle
   # 应返回一个毫秒数
   ```

2. **检查应用是否在运行**：
   ```bash
   ps aux | grep purr-pause
   ```

3. **手动触发测试**：
   右键点击托盘图标 → "立即测试"

#### 视频不显示 / 黑色背景

- 确认视频文件为 WebM 格式且包含 Alpha 通道
- 检查 GPU 加速是否正常：
  ```bash
  # 启动时查看 GPU 信息
  /opt/purr-pause/purr-pause --enable-logging --v=1
  ```

#### 托盘图标不显示

- GNOME 桌面需安装 AppIndicator 扩展：
  ```bash
  sudo apt install gnome-shell-extension-appindicator
  ```
- 安装后注销并重新登录

#### 设置无法保存

- 检查配置目录权限：
  ```bash
  ls -la ~/.config/purr-pause/
  # 确保当前用户有写权限
  chmod 755 ~/.config/purr-pause/
  chmod 644 ~/.config/purr-pause/config.json
  ```

### 7.2 重置为默认设置

```bash
rm ~/.config/purr-pause/config.json
# 下次打开设置时会使用内置默认值
```

## 8. 日志位置

应用日志记录在 `~/.config/purr-pause/logs/` 目录下，时间戳使用北京时间（UTC+8）。

单个日志文件最大 128KB，超出后自动轮转，最多保留 5 个历史文件。

查看日志：

```bash
# 通过托盘菜单"查看日志"直接打开日志文件

# 或手动查看
cat ~/.config/purr-pause/logs/purr-pause.log

# 如果使用 systemd
journalctl --user -u purr-pause -f

# Electron 崩溃报告
ls ~/.config/purr-pause/Crashpad/
```

## 9. 从源码构建

### 9.1 环境准备

```bash
# 安装 Node.js 18+
curl -fsSL https://deb.nodesource.com/setup_18.x | sudo -E bash -
sudo apt install -y nodejs

# 验证
node --version  # >= 18.0.0
npm --version   # >= 9.0.0
```

### 9.2 克隆与安装依赖

```bash
git clone https://github.com/purr-pause/purr-pause.git
cd purr-pause
npm install
```

### 9.3 开发运行

```bash
# 正常启动
npm start

# 开发模式（9 秒触发，方便测试）
npm run dev
```

### 9.4 构建 Linux 包（deb / AppImage，x64 / arm64）

```bash
# 当前主机架构的 deb（默认）
npm run build:linux

# arm64 的 deb（x64 主机可交叉打包，无需 ARM 机器）
npm run build:linux:arm64

# AppImage（x64 / arm64）
npm run build:appimage
npm run build:appimage:arm64

# 一次性产出全部 4 个 Linux 产物（deb + AppImage，x64 + arm64）
npm run build:linux:all

# 输出位于 dist/ 目录
ls dist/*.deb dist/*.AppImage
```

### 9.5 构建其他平台

```bash
# macOS
npm run build:mac

# Windows（x64 + arm64）
npm run build:win:all

# 全平台
npm run build:all
```

## 10. CI/CD (GitHub Actions)

### 10.1 当前工作流配置

当前仓库的 `.github/workflows/build.yml` 会在推送 `v*` tag 时构建 Linux（deb + AppImage，x64 + arm64）、macOS 和 Windows 产物，并创建 GitHub Release：

```yaml
name: Build & Release

on:
  push:
    tags:
      - 'v*'

jobs:
  build-linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Install dependencies
        run: npm install
        env:
          ELECTRON_MIRROR: https://npmmirror.com/mirrors/electron/

      - name: Install build tools
        run: sudo apt-get install -y xz-utils binutils

      - name: Build deb + AppImage (x64 + arm64)
        run: npm run build:linux:all
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: purr-pause-linux
          path: |
            dist/*.deb
            dist/*.AppImage

  build-mac:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Install dependencies
        run: npm install

      - name: Build
        run: npm run build:mac -- --arm64 --x64
        env:
          CSC_IDENTITY_AUTO_DISCOVERY: false
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: purr-pause-mac
          path: dist/*.dmg

  build-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Install dependencies
        run: npm install

      - name: Build x64 + arm64
        run: npm run build:win:all
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: purr-pause-windows
          path: dist/*.exe

  release:
    needs: [build-linux, build-mac, build-windows]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - name: Download all artifacts
        uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Create Release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            artifacts/purr-pause-linux/*
            artifacts/purr-pause-mac/*
            artifacts/purr-pause-windows/*
          generate_release_notes: true
```

### 10.2 版本发布流程

```bash
# 1. 确认 package.json/package-lock.json 版本号已更新

# 2. 推送标签触发构建
git tag v1.4.0
git push origin --tags
```

## 11. 数据目录总览

| 路径 | 内容 |
|------|------|
| `~/.config/purr-pause/config.json` | 用户配置 |
| `~/.config/purr-pause/webm/` | 自定义视频素材 |
| `~/.config/purr-pause/license.json` | 授权状态 |
| `~/.config/purr-pause/.install_mark` | 试用期安装标记 |
| `~/.config/purr-pause/logs/` | 应用日志 |
| `~/.config/purr-pause/Crashpad/` | 崩溃报告 |
| `~/.config/autostart/purr-pause.desktop` | 用户级自启动条目 |
| `~/.local/share/.purr-pause-mark` | 试用期辅助标记 |
