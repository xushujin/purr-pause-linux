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
- 机器码绑定的序列号激活系统（7 天免费试用）
- 系统托盘常驻，不占用任务栏
- 单实例运行，重复启动会立即退出
- 消息提醒（待办通知，可选模块，默认关闭）：轮询第三方待办接口，有新待办时提醒并在托盘显示角标

## 安装

### Debian / Ubuntu

```bash
sudo dpkg -i purr-pause-linux_1.0.0_amd64.deb
```

依赖：`libgtk-4-1`、`libadwaita-1-0`、`libmpv2`（安装时会自动检查）

> **GNOME 用户注意**：GNOME 默认不提供系统托盘宿主，需安装扩展：
> ```bash
> sudo apt install gnome-shell-extension-appindicator
> ```
> KDE Plasma、Sway、Hyprland、XFCE 等开箱即用。

### 从源码构建

```bash
# Debian / Ubuntu 构建依赖
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
libmpv-dev libssl-dev dpkg-dev imagemagick desktop-file-utils

cd native
./build-deb.sh
# 产物：native/dist/purr-pause-linux_<版本>_amd64.deb
```

## 使用

1. 安装后自动在系统托盘显示肥猫图标
2. 右键托盘图标可以：待办消息、暂停监控、设置、激活/续期、查看日志、立即测试、退出
3. 托盘菜单显示激活状态、距下次休息的剩余时间和软件版本号
4. 屏幕使用时间到达阈值后，猫咪自动出现在所有显示器上
5. 可点击"再等 5 分钟"延后休息（最多 2 次）
6. 点击"精力充沛！"按钮结束休息；倒计时结束后会自动关闭

## 设置

右键托盘图标 → 设置：

| 选项 | 说明 | 范围 |
|------|------|------|
| 屏幕使用时间 | 连续使用多久后提醒 | 1-480 分钟 |
| 休息时长 | 每次休息持续多久 | 1-60 分钟 |
| 动画模式 | 猫咪出现的方式（切换需激活） | 中央走出 / 右下角走出 / 中央淡入 / 右侧走出 |
| 素材目录 | 自定义视频素材路径（需激活） | — |
| 开机自动启动 | 写入 `~/.config/autostart/purr-pause-linux.desktop` | 开/关 |
| 写入日志 | 记录计时、激活与消息提醒过程，输出到 `~/.local/share/purr-pause-linux/logs/` | 开/关 |

设置窗口底部会显示当前的**空闲检测方式**，便于排查计时不准的问题。

## 自定义素材

将 webm 视频放到 `~/.local/share/purr-pause-linux/webm/` 目录下：

- `cat-walk.webm` — 走路动画（播放一次）
- `cat-rest.webm` — 躺下动画（循环播放）

要求：WebM 格式，VP9 编码，带 Alpha 通道（透明背景）。
制作参考见 `native/data/README-assets.txt`。

## 消息提醒（待办通知）

可选模块，**默认关闭**。开启后胖猫会轮询你配置的第三方待办接口
（工单 / 审批 / 告警 / 任务等），有新待办时提醒你，并在托盘显示待办角标。
应用只做轮询、提醒、展示与跳转，不保存业务数据。

第三方接口对接规范见 [docs/message-api-spec.md](docs/message-api-spec.md)。

要点：
- 每个源配置完整 URL 与请求头，轮询 `GET <url>?limit=<N>&offset=0`
- 首次同步只建立基线、不弹提醒；之后按 `id` 差集检测新增
- 连续失败 5 次后自动降频退避（最高 300 秒）
- 点击某条待办用默认浏览器打开，并在本地清理（不回写第三方）
- 仅允许打开 `http(s)` 链接；请求头不写日志

## 序列号

### 激活流程

1. 用户在激活窗口中复制自己的 **机器码**（16 位）
2. 将机器码发送给开发者
3. 开发者生成绑定该机器的序列号
4. 用户输入序列号完成激活

序列号格式：以 `PP-` 为前缀，后接 Base64 编码数据，并按每 8 个字符用 `-` 分隔。
每个序列号绑定特定机器，不可在其他设备上使用。

### 生成序列号（开发者）

> ⚠️ `tools/keygen.js` 含 Ed25519 发号私钥，**已被 `.gitignore` 排除，不随仓库分发**。
> 从私有备份恢复后再使用：
> ```bash
> cp ~/purr-pause-backup-private/keygen.js tools/
> ```

```bash
node tools/keygen.js --mid <机器码> --days 30   # 30 天
node tools/keygen.js --mid <机器码> --days 365  # 1 年
node tools/keygen.js --mid <机器码> --permanent # 永久
node tools/keygen.js --verify <序列号>          # 验证
```

> **切勿将 `keygen.js` 提交到公开仓库。** 私钥一旦公开，任何人都能生成
> 任意天数的序列号绕过付费。公钥部分在 `native/src/license/mod.rs` 中，是公开的。

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

### 序列号一致性校验

序列号的签发端（`tools/keygen.js`）与验证端（`native/src/license/`）
必须产生完全一致的字节，否则用户会无法激活。

改动激活逻辑后务必跑：

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
│       ├── main.rs          # 入口：托盘、计时、覆盖层、消息轮询
│       ├── license/         # 序列号验证与激活
│       ├── core/            # 配置 / 空闲检测 / 日志 / 开机自启
│       ├── message/         # 待办轮询与差集检测
│       ├── ui/              # 覆盖层 / 设置 / 激活 / 待办列表 / 托盘
│       └── video.rs         # libmpv 视频解码（Cairo backend）
├── tools/
│   ├── keygen.js            # 序列号生成器（含私钥，已 gitignore，需从私有备份恢复）
│   ├── sig-fixtures/        # 跨语言签名等价性回归测试
│   └── uninstall-linux.sh   # 卸载脚本
├── docs/
│   └── message-api-spec.md  # 第三方待办接口对接规范
└── .github/workflows/
    └── build-native.yml     # CI：Ubuntu 24.04 构建 deb 并发布 Release
```

## 技术栈

- GTK4 / libadwaita —— 窗口与控件
- Rust —— 业务逻辑
- libmpv —— webm（VP9 + Alpha）解码，经 Cairo backend 直接绘制
- reqwest —— 第三方待办接口轮询
- ed25519-dalek —— 序列号签名验证
- AppIndicator —— 系统托盘

**无原生扩展依赖**，除系统 GTK4 与 libmpv 外全部为纯 Rust 实现。

## 致谢

本应用主要通过 vibe coding 方式完成，即由开发者提出产品需求、交互细节与实现方向，
并借助 AI 编程工具协助完成代码实现、调试与文档整理。

本项目为 Linux 原生实现（GTK4 + libadwaita），常驻内存约 30MB，冷启动约 90ms。

内置猫咪视频素材由 GitHub 用户 [louiscmli0719](https://github.com/louiscmli0719)
协助完成。感谢其在素材制作与整理上的贡献。

## License

MIT