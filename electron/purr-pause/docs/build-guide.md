# 胖猫暂停一下（PurrPause） 打包指南

## 环境要求

| 平台 | 系统要求 | 工具要求 |
|------|---------|---------|
| Linux (.deb / AppImage) | Ubuntu 18.04+ 或任意 Linux（x64 / arm64） | Node.js 18+, npm, `xz-utils`, `binutils` |
| macOS (.dmg) | macOS 11+ | Node.js 18+, npm, Xcode Command Line Tools |
| Windows (.exe) | Windows 10+（x64 / arm64） | Node.js 18+, npm |

## 通用步骤

### 1. 克隆项目并安装依赖

```bash
git clone <your-repo-url>
cd purr-pause
npm install
```

> 如果 Electron 下载慢，可使用镜像：
> ```bash
> ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/ npm install
> ```

### 2. 验证开发环境

```bash
npm start
```

确认应用正常启动，托盘图标出现后再进行打包。

---

## Linux 打包 (.deb / AppImage)

### 额外依赖

```bash
sudo apt-get install -y xz-utils binutils
```

### 打包命令

```bash
# 当前主机架构的 deb（默认）
npm run build:linux

# arm64 的 deb（x64 主机可交叉打包，无需 ARM 机器）
npm run build:linux:arm64

# AppImage（x64 / arm64，免安装、双击即用）
npm run build:appimage
npm run build:appimage:arm64

# 一次性产出全部 4 个 Linux 产物（deb + AppImage，x64 + arm64）
npm run build:linux:all
```

> 本项目无原生依赖，可在 x64 主机上交叉打包 arm64：electron-builder 会自动下载对应架构的 Electron 预编译包；AppImage 所需的 appimagetool 也会在构建时自动下载，无需额外 apt 包。

### 产物位置

```
dist/purr-pause_1.4.0_amd64.deb           # x64 deb
dist/purr-pause_1.4.0_arm64.deb           # arm64 deb
dist/purr-pause-1.4.0.AppImage            # x64 AppImage
dist/purr-pause-1.4.0-arm64.AppImage      # arm64 AppImage
```

### 安装测试

```bash
sudo dpkg -i dist/purr-pause_1.4.0_amd64.deb
```

通过 `sudo dpkg -i` 安装时，deb 的 `afterInstall` 脚本会先把 `/opt/purr-pause/chrome-sandbox` 设置为 `root:root` 和 `4755`，避免 Electron 启动时报 SUID sandbox 权限错误；随后为执行 sudo 的桌面用户写入 `~/.config/autostart/purr-pause.desktop`，用于 Ubuntu 登录后自启动。应用内设置页的“开机自动启动”开关读写同一个文件。

### 卸载

```bash
sudo dpkg -r purr-pause
```

---

## macOS 打包 (.dmg)

### 前置条件

1. 安装 Xcode Command Line Tools：
   ```bash
   xcode-select --install
   ```

2. （可选）配置代码签名：
   - 如果不签名，用户首次打开需要右键 → 打开 → 确认
   - 如果需要签名，设置环境变量：
     ```bash
     export CSC_NAME="Developer ID Application: Your Name (TEAM_ID)"
     ```

### 打包命令

```bash
npm run build:mac
```

### 产物位置

```
dist/purr-pause-1.4.0.dmg
dist/purr-pause-1.4.0-arm64.dmg  (Apple Silicon)
```

### 安装测试

双击 .dmg 文件，将应用拖入 Applications 文件夹。

### 注意事项

- 默认打包当前架构（Intel 或 Apple Silicon）
- 如需同时打包两种架构（与当前 GitHub Actions 工作流一致）：
  ```bash
  npm run build:mac -- --arm64 --x64
  ```
- 未签名的应用首次运行需要：系统偏好设置 → 安全性与隐私 → 仍要打开

---

## Windows 打包 (.exe)

### 打包命令

```bash
# 当前主机架构（默认）
npm run build:win

# arm64（x64 主机可交叉打包，无需 ARM 机器）
npm run build:win:arm64

# x64 + arm64（一次产出两个安装包）
npm run build:win:all
```

### 产物位置

```
dist/purr-pause-Setup-1.4.0-x64.exe      # x64
dist/purr-pause-Setup-1.4.0-arm64.exe    # arm64
```

### 安装测试

双击 .exe 安装包，按提示完成安装。

### 注意事项

- 未签名的安装包会触发 Windows SmartScreen 警告
- 用户需要点击"更多信息" → "仍要运行"
- 如需代码签名，设置环境变量：
  ```bash
  set CSC_LINK=path/to/certificate.pfx
  set CSC_KEY_PASSWORD=your-password
  ```

---

## 使用 GitHub Actions 自动构建（推荐）

仓库已包含 `.github/workflows/build.yml`。该工作流会在推送 `v*` tag 时分别构建 Linux（deb + AppImage，x64 + arm64）、macOS、Windows，并创建 GitHub Release：

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

### 使用方法

```bash
git tag v1.4.0
git push origin v1.4.0
```

推送 tag 后 GitHub Actions 会自动在三个平台上构建，构建完成后在 Actions 页面下载产物。

---

## 常见问题

### Q: Linux 打包报错 "Need executable 'ar'"
```bash
sudo apt-get install -y binutils
```

### Q: Linux 打包报错 "tar failed (exit code 2)"
```bash
sudo apt-get install -y xz-utils
```

### Q: Electron 下载超时
使用镜像源：
```bash
ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/ npm install
```

### Q: macOS 打包报错 "No identity found for signing"
不需要签名时跳过：
```bash
CSC_IDENTITY_AUTO_DISCOVERY=false npm run build:mac
```

### Q: Windows 打包在 Linux 上失败
Windows 打包需要 wine 或在 Windows 系统上执行。推荐使用 GitHub Actions 自动构建。

---

## 项目结构

```
purr-pause/
├── main.js              # Electron 主进程
├── preload.js           # IPC 桥接
├── config.json          # 默认配置
├── package.json         # 项目配置 & 打包配置
├── lib/
│   ├── license.js       # 激活/授权逻辑
│   ├── message-notify.js # 消息提醒
│   └── logger.js        # 日志
├── renderer/
│   ├── index.html            # 猫咪动画覆盖层
│   ├── settings.html         # 设置窗口
│   ├── activation.html       # 激活窗口
│   ├── message-list.html     # 消息列表
│   ├── message-settings.html # 消息提醒设置
│   ├── rocket-demo.html      # 小火箭演示
│   └── rules.html            # 规则说明
├── assets/
│   ├── images/
│   │   ├── logo.png
│   │   └── tray-icon.png
│   └── webm/
│       ├── cat-walk.webm   # 走路动画
│       └── cat-rest.webm   # 躺下动画
└── build/
    └── icons/           # 各平台图标
        ├── icon.ico     # Windows
        ├── icon.icns    # macOS
        └── *.png        # Linux (多尺寸)
```
