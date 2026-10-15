# 胖猫暂停一下（PurrPause）

一只胖猫在你盯屏幕太久时走出来挡住你的屏幕，提醒你暂停一下。

## 功能

- 监控屏幕使用时间，超时后肥猫从屏幕边缘走到中央
- 四种动画模式：中央走出（默认）、右下角走出（放大）、中央淡入、右侧走出（切换需激活）
- 猫咪从小变大，走完后躺下并自动适配屏幕宽高
- 倒计时休息，结束后猫咪渐变消失
- 多显示器支持，猫咪同时出现在所有屏幕
- 暂停监控（30 分钟 / 1 小时 / 2 小时 / 手动恢复）
- 延后休息（"再等 5 分钟"，每次最多 2 次）
- 托盘菜单显示距下次休息剩余时间
- 开机自动启动
- 支持自定义 webm 视频素材（激活用户可自定义目录）
- 机器码绑定的序列号激活系统（7 天免费试用）
- 系统托盘常驻，不占用任务栏
- 单实例运行，重复启动会立即退出，不会创建第二个托盘进程
- 消息提醒（待办通知，可选模块，默认关闭）：轮询你配置的第三方待办接口，有新待办时小火箭从屏幕角落飞出提醒，并在托盘显示待办角标
- 内置“待办消息”列表窗口，支持多接口源（彩色来源标签），点击跳转第三方页面并本地清理

## 安装

### Ubuntu / Debian

```bash
sudo dpkg -i dist/purr-pause_1.4.0_amd64.deb
```

### 从源码运行

```bash
git clone <repo-url>
cd purr-pause
npm install
npm start
```

## 使用

1. 安装后自动在系统托盘显示肥猫图标
2. 右键托盘图标可以：待办消息、消息提醒设置、暂停监控、设置、计时规则、查看日志、立即测试、激活/续期、退出
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
| 动画模式 | 猫咪出现的方式（切换需激活） | 中央走出（默认） / 右下角走出 / 中央淡入 / 右侧走出 |
| 素材目录 | 自定义视频素材路径（需激活） | — |
| 开机自动启动 | 系统启动时自动运行；Ubuntu 下写入 `~/.config/autostart/purr-pause.desktop` | 开/关 |

## 自定义素材

将 webm 视频放到 `~/.config/purr-pause/webm/` 目录下：

- `cat-walk.webm` — 走路动画（播放一次）
- `cat-rest.webm` — 躺下动画（循环播放）

`walk-zoom`、`walk-flat`、`walk-center` 使用两个视频；`fade-center` 只使用 `cat-rest.webm`。

要求：WebM 格式，VP9 编码，带 Alpha 通道（透明背景）。

## 消息提醒（待办通知）

可选模块，**默认关闭**。开启后胖猫会轮询你配置的第三方待办接口（工单 / 审批 / 告警 / 任务等），有新待办时用“小火箭”轻量动画从屏幕角落飞出提醒你，并在托盘显示待办角标。应用只做轮询、提醒、展示与跳转，不保存业务数据。

托盘菜单提供两个入口：

- **待办消息（N）** — 打开“待办消息”列表窗口，N 为当前待办角标数。
- **消息提醒设置** — 打开独立的消息提醒设置窗口（与休息提醒的“设置”分开）。

### 设置项（消息提醒设置窗口）

| 选项 | 说明 |
|------|------|
| 启用消息提醒 | 总开关；关闭后停止所有源轮询并隐藏角标，但保留已填写的配置 |
| 接口源 | 可添加一个或多个第三方接口源，每个源含：源名称、待办接口地址（完整 URL）、请求头（JSON，如 `{"Authorization":"Bearer xxx"}`）、启用开关、测试连接 |
| 轮询间隔 | 拉取待办列表的间隔，所有源共用，10–3600 秒（默认 60） |
| 每源待办条数 | 每个源查询并保留的最近待办条数，1–30（默认 30），同时决定列表显示与角标上限 |
| 小火箭素材目录 / 小火箭素材 | 自定义提醒动画素材（见下方“小火箭素材”） |
| 📄 下载接口规范 | 导出第三方对接文档 `PurrPause-消息提醒接口规范-v1.md` |

### 工作方式

- 按固定间隔轮询每个启用源的待办接口（`GET <接口地址>?limit=<N≤30>&offset=0`，第三方需按 `createdAt` 倒序返回约定的 JSON）。
- 用返回项的 `id` 与本地已知集合比对发现新待办：**首次同步只建立基线、不弹动画**；之后检测到新增才播放一次小火箭并刷新角标。
- “待办消息”列表按彩色来源标签区分多个接口源；点击某条用系统默认浏览器打开其 `targetUrl`，并在本地清理该条（角标 -1、列表移除），不回写第三方。
- 角标取本地保留的待办条数（各启用源 ≤30 之和），不取自服务端 `total`/`unread`。
- 休息提醒优先级更高：休息覆盖层显示期间不弹小火箭（仍更新角标），结束后合并补播一次。
- 某个源连续失败 5 次后自动降频退避（间隔翻倍，最高 300 秒），任意一次成功后恢复。
- 仅允许打开 `http(s)` 链接；列表用纯文本渲染、请求头（可能含 token）不写日志、消息正文不落盘。

第三方接口对接规范见 [docs/message-api-spec.md](docs/message-api-spec.md)（也可在设置窗口点“下载接口规范”导出）。

### 小火箭素材

消息提醒设置中可以单独选择“小火箭素材目录”。保存后，目录下会自动生成 `purr-pause-消息提醒素材说明.txt`。

默认文件名：

- `notify-rocket.webm` — 小火箭消息提醒动画（找不到时回退到内置 SVG 动画）

查找顺序：小火箭素材目录 → 通用素材目录 → `~/.config/purr-pause/webm/` → 应用内置素材。

## 序列号

### 激活流程

1. 用户在激活窗口中查看并复制自己的 **机器码**（16 位）
2. 将机器码发送给开发者
3. 开发者根据机器码生成绑定该机器的序列号
4. 用户输入序列号完成激活

序列号格式：以 `PP-` 为前缀，后接 Base64 编码数据，并按每 8 个字符用 `-` 分隔。

每个序列号绑定特定机器，不可在其他设备上使用。

### 生成序列号（开发者）

```bash
node tools/keygen.js --mid <机器码> --days 30        # 30 天
node tools/keygen.js --mid <机器码> --days 365       # 1 年
node tools/keygen.js --mid <机器码> --permanent      # 永久
node tools/keygen.js --verify <序列号>               # 验证
```

### 激活

右键托盘图标 → 激活/续期，复制机器码发给开发者，获取序列号后输入激活。

## 打包

### 打包 Linux 包（deb / AppImage，x64 / arm64）

一键脚本：自动定位项目根目录、检查 node/npm/binutils、缺依赖时自动 `npm install`，结束后列出全部产物与大小。

```bash
./scripts/build-deb.sh                 # 当前主机架构的 deb（默认）
./scripts/build-deb.sh --arm64         # arm64 的 deb（x64 主机可交叉打包，无需 ARM 机器）
./scripts/build-deb.sh --appimage      # 同时产出 AppImage（与 deb 一起）
./scripts/build-deb.sh --all           # deb + AppImage，x64 + arm64（共 4 个产物）
bash scripts/build-deb.sh              # 没有执行权限时这样跑
```

- 产物位于 `dist/`：`purr-pause_<版本>_amd64.deb`、`purr-pause_<版本>_arm64.deb`、`purr-pause-<版本>.AppImage`、`purr-pause-<版本>-arm64.AppImage`（版本号取自 `package.json`）。
- 本项目无原生依赖，可在 x64 主机上交叉打包 arm64；额外参数会原样透传给 electron-builder，例如：`./scripts/build-deb.sh --publish never`。

### 其他平台

```bash
npm run build:linux:all   # Linux deb + AppImage，x64 + arm64（一次产出 4 个）
npm run build:mac         # macOS dmg (需在 Mac 上)
npm run build:win:all     # Windows exe x64 + arm64 (需在 Windows 上)
```

推送 `v*` tag 时，GitHub Actions 会自动在三平台构建并发布 Release（Linux 含 deb + AppImage、x64 + arm64）。详细打包说明见 [docs/build-guide.md](docs/build-guide.md)。

## 技术栈

- Electron 33
- 纯 HTML/CSS/JS（无框架）
- WebM 视频 + CSS 动画
- Ed25519 序列号签名

## 项目结构

```
purr-pause/
├── main.js              # Electron 主进程
├── preload.js           # IPC 桥接
├── lib/
│   ├── license.js       # 序列号验证 & 激活管理
│   ├── message-notify.js # 消息提醒（小火箭）轮询与动画
│   └── logger.js        # 日志
├── tools/keygen.js      # 序列号生成器（CLI）
├── renderer/
│   ├── index.html       # 猫咪动画覆盖层
│   ├── settings.html    # 设置窗口
│   ├── activation.html  # 激活窗口
│   ├── message-list.html # 待办消息列表
│   ├── message-settings.html # 消息提醒设置
│   ├── rocket-demo.html # 小火箭演示
│   └── rules.html       # 计时规则说明
├── assets/
│   ├── images/
│   │   ├── logo.png          # 应用 logo 源图
│   │   └── tray-icon.png     # 系统托盘图标
│   └── webm/
│       ├── cat-walk.webm
│       └── cat-rest.webm
├── config.json          # 默认配置
├── docs/                # 文档
└── .github/workflows/   # CI/CD
```

## 致谢

本应用主要通过 vibe coding 方式完成，即由开发者提出产品需求、交互细节与实现方向，并借助 AI 编程工具协助完成代码实现、调试与文档整理。

内置猫咪视频素材由 GitHub 用户 [louiscmli0719](https://github.com/louiscmli0719) 协助完成。感谢其在素材制作与整理上的贡献。

## License

MIT
