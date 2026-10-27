# 胖猫暂停一下（PurrPause Linux）

一只胖猫在你盯屏幕太久时走出来挡住你的屏幕，提醒你暂停一下。

基于 **GTK4 + libadwaita** 的 Linux 原生应用。
常驻内存约 30MB，冷启动约 90ms。

## 功能

- 监控屏幕使用时间，超时后肥猫从屏幕边缘走到中央
- 四种动画模式：中央走出（默认）、右下角走出（放大）、中央淡入、右侧走出
- 猫咪从小变大，走完后躺下并自动适配屏幕宽高
- 倒计时休息，结束后猫咪渐变消失
- 多显示器支持，猫咪同时出现在所有屏幕
- 暂停监控（30 分钟 / 1 小时 / 2 小时 / 手动恢复）
- 延后休息（"再等 5 分钟"，每次最多 2 次）
- 托盘菜单显示距下次休息剩余时间
- 开机自动启动
- 支持自定义 webm 视频素材
- **安装即全功能**：无授权码、无试用期限制
- 系统托盘常驻，不占用任务栏
- 单实例运行，重复启动会立即退出

## 安装

### Debian / Ubuntu

```bash
sudo dpkg -i purr-pause-linux_1.0.0_amd64.deb
sudo apt-get -f install   # 若有依赖未装，用它补齐
```

**系统要求**：GTK **4.6** 及以上、libadwaita **1.1** 及以上

| 发行版 | 自带版本 | 是否支持 |
|---|---|---|
| Ubuntu 22.04 LTS | GTK 4.6 / libadwaita 1.1 | ✅ |
| Debian 12 (bookworm) | GTK 4.8 / libadwaita 1.2 | ✅ |
| Ubuntu 24.04 LTS | GTK 4.14 / libadwaita 1.5 | ✅ |
| Debian 11、Ubuntu 20.04 | GTK 4.0 以下 / 无 GTK4 | ❌ |

依赖：`libgtk-4-1`、`libadwaita-1-0`、`libmpv2` **或** `libmpv1`
（安装时由包管理器自动检查与补齐；libmpv 在运行时动态加载，两代都可用）

> **GNOME 用户注意**：GNOME 默认不提供系统托盘宿主，需安装扩展：
> ```bash
> sudo apt install gnome-shell-extension-appindicator
> ```
> KDE Plasma、Sway、Hyprland、XFCE 等开箱即用。

### 从源码构建

> ⚠️ **构建机要用最老的目标发行版**（本项目是 Ubuntu 22.04）。
> glibc 的符号版本只向下兼容：在 24.04（glibc 2.39）上构建的产物在
> 22.04（glibc 2.35）上会直接以 `version 'GLIBC_2.38' not found` 启动失败，
> **改代码无法解决，只能换构建环境**。CI 对此有守卫：打包后检查二进制的
> glibc 符号版本不超过 2.35。

```bash
# Debian / Ubuntu 构建依赖（注意：不需要 libmpv-dev，本应用运行时才加载 libmpv）
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  libssl-dev dpkg-dev imagemagick desktop-file-utils

cd native
./build-deb.sh
# 产物：native/dist/purr-pause-linux_<版本>_amd64.deb
```

## 使用

1. 安装后自动在系统托盘显示肥猫图标
2. 右键托盘图标可以：暂停 30 分钟 / 1 小时 / 2 小时 / 到手动恢复、设置、查看日志、立即测试、退出
3. 托盘菜单显示距下次休息的剩余时间和软件版本号
4. 屏幕使用时间到达阈值后，猫咪自动出现在所有显示器上
5. 可点击"再等 5 分钟"延后休息（最多 2 次）
6. 点击"精力充沛！"按钮结束休息；倒计时结束后会自动关闭
7. 覆盖层随时可以用 **Esc**、**双击画面** 或 **关闭按钮** 退出

### 命令行入口（托盘不可用时必备）

```bash
purr-pause-linux --test        # 立刻弹一次休息提醒，验证屏保是否正常
purr-pause-linux --settings    # 直接打开设置窗口（不依赖托盘）
purr-pause-linux --selftest    # 打印环境自检报告（排查「点了没反应」）
purr-pause-linux --quit        # 让已在运行的实例退出
purr-pause-linux --version     # 版本
```

GNOME 默认不显示第三方托盘图标（需要 `gnome-shell-extension-appindicator`）。
没有托盘时，用 `--settings` 打开设置、用 `--test` 验证屏保、用 `--quit` 退出。

## 设置

右键托盘图标 → 设置（或运行 `purr-pause-linux --settings`）：

| 选项 | 说明 | 范围 |
|------|------|------|
| 屏幕使用时间 | 连续使用多久后提醒 | 1-480 分钟 |
| 休息时长 | 每次休息持续多久 | 1-60 分钟 |
| 动画模式 | 猫咪出现的方式 | 中央走出 / 右下角走出 / 中央淡入 / 右侧走出 |
| 素材目录 | 自定义视频素材路径（可直接输入或浏览选择） | — |
| 开机自动启动 | 写入 `~/.config/autostart/purr-pause-linux.desktop` | 开/关 |
| 写入日志 | 记录计时过程，输出到 `~/.local/share/purr-pause-linux/logs/` | 开/关 |

点「保存」后窗口不会关闭，顶部会弹出「设置已保存」提示条 ——
看到提示就说明配置已经写进 `~/.local/share/purr-pause-linux/config.json`。

设置窗口顶部还有「立即测试猫咪」和「打开日志目录」两个快捷入口。

## 排查问题

先跑一次自检，把输出整段复制出来：

```bash
purr-pause-linux --selftest
```

它会逐项报告：系统与会话类型、GTK 版本、显示器、libmpv 是否可加载、
素材文件是否存在、配置目录是否可写、空闲检测方式、托盘宿主是否存在。

屏保相关的额外线索：覆盖层里如果出现「视频渲染不可用，已降级为静态猫咪」之类的
提示，说明 mpv / OpenGL 那一环有问题，但应用会继续以静态猫图工作，不会黑屏。


## 自定义素材

将 webm 视频放到 `~/.local/share/purr-pause-linux/webm/` 目录下：

- `cat-walk.webm` — 走路动画（播放一次）
- `cat-rest.webm` — 躺下动画（循环播放）

要求：WebM 格式，VP9 编码。

**关于背景**：内置素材其实**不带 alpha 通道**，而是纯绿底（`#00AE3E`），
应用在播放时用 libavfilter 的 `colorkey` 实时抠掉这个绿色。
自备素材两种都可以：
- 直接给带 Alpha 通道的 WebM（推荐，抠像滤镜对它无副作用）；
- 或者沿用同样的纯绿底 `#00AE3E`。

制作参考见 `native/data/README-assets.txt`。

## 授权

**native 版（Linux）已移除授权功能**：安装即全功能，没有机器码、没有序列号、
没有试用期，所有设置项都可以直接修改保存。

`tools/keygen.js` 与 `tools/sig-fixtures/` 仅保留给 Electron 版使用，与 native 版无关。

## 开发

```bash
cd native

# 调试运行
cargo run

# 交叉编译（在 macOS 上需要先准备 Linux sysroot）
./setup-sysroot.sh
export PKG_CONFIG_SYSROOT_DIR=$PWD/target/sysroot
export PKG_CONFIG_PATH=$PWD/target/sysroot/usr/lib/x86_64-linux-gnu/pkgconfig
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu
```

### 序列号一致性校验（Electron 版，与 native 无关）

Electron 版的签发端（`tools/keygen.js`）与验证端必须产生完全一致的字节。
改动那套逻辑后跑：

```bash
cd tools/sig-fixtures
mkdir -p src && cp verify.rs src/main.rs
cargo run --quiet
rm -rf src target Cargo.lock
```

该脚本有两种模式，均以 `PASS` 结尾：

| 模式 | 条件 | 覆盖 |
|------|------|------|
| 完整校验 | 本机有 `fixtures.json`（含私钥，从私有备份恢复） | 公钥推导、签名字节逐字节比对、序列号字符串比对、解析回测 |
| 无密钥自检 | 无 `fixtures.json`（公开仓库 CI 即此模式） | 解析顺序、验签链路、抗篡改回归 |

详见 [tools/sig-fixtures/README.md](tools/sig-fixtures/README.md)。

CI 会在每次 push 到主干、PR、以及打 `v*` tag 时自动执行该测试，
并在构建后校验 deb 的 ar 结构与关键文件是否齐全。

## 项目结构

```
purr-pause-linux/
├── native/                  # Rust / GTK4 应用主体
│   ├── Cargo.toml
│   ├── build-deb.sh         # deb 打包脚本
│   ├── setup-sysroot.sh     # macOS 交叉编译用的 Linux sysroot
│   ├── debian/              # deb 打包配置（control/postinst/prerm/desktop）
│   ├── data/                # 内置素材与默认配置
│   │   ├── config.json
│   │   ├── webm/            # cat-walk.webm / cat-rest.webm
│   │   └── icons/
│   └── src/
│       ├── main.rs          # 入口：托盘、计时、覆盖层
│       ├── core/            # 配置 / 空闲检测 / 日志 / 开机自启
│       ├── ui/              # 覆盖层 / 设置 / 托盘
│       └── video.rs         # libmpv 解码 + 渲染到 GtkGLArea（运行时 dlopen libmpv）
├── tools/
│   ├── keygen.js            # Electron 版序列号生成器（native 版已不需要）
│   ├── sig-fixtures/        # Electron 版跨语言签名等价性回归测试
│   └── uninstall-linux.sh   # 卸载脚本
└── .github/workflows/
    └── build-native.yml     # CI：24.04 构建 deb、22.04 容器内验证安装、打 tag 发 Release
```

## 技术栈

- GTK4 / libadwaita —— 窗口与控件（最低 4.6 / 1.1，代码中不使用更高版本才有的 API）
- Rust —— 业务逻辑
- libmpv —— webm（VP9 + Alpha）解码，经 render API 渲染进 GtkGLArea；
  运行时 dlopen（`libmpv.so.2` → `libmpv.so.1`），不绑定具体版本
- 自实现 D-Bus StatusNotifierItem —— 系统托盘（GTK4 下无法再用 libappindicator）

**无原生扩展依赖**，除系统 GTK4 与 libmpv 外全部为纯 Rust 实现。

## 致谢

本应用主要通过 vibe coding 方式完成，即由开发者提出产品需求、交互细节与实现方向，
并借助 AI 编程工具协助完成代码实现、调试与文档整理。

本项目为 Linux 原生实现（GTK4 + libadwaita），常驻内存约 30MB，冷启动约 90ms。

内置猫咪视频素材由 GitHub 用户 [louiscmli0719](https://github.com/louiscmli0719)
协助完成。感谢其在素材制作与整理上的贡献。

## License

MIT