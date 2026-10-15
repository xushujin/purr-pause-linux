# 胖猫暂停一下（PurrPause） 技术开发文档

## 1. 项目概述

胖猫暂停一下（PurrPause） 是一款基于 Electron 的桌面应用程序，运行于 Linux、macOS 和 Windows 系统。当用户连续使用电脑超过设定时间后，一只胖猫会走上屏幕，提醒用户休息。

- **应用名称**: 胖猫暂停一下（PurrPause）
- **版本**: 1.4.0
- **许可证**: MIT
- **应用 ID**: com.purr-pause.app

## 2. 架构概述

### 2.1 Electron 进程模型

应用采用 Electron 标准的多进程架构：

```
┌─────────────────────────────────────────────┐
│              Main Process (主进程)            │
│  main.js                                     │
│  - 空闲检测                                   │
│  - 配置管理                                   │
│  - 系统托盘                                   │
│  - 窗口管理                                   │
│  - 多显示器处理                               │
└──────────┬──────────────────┬───────────────┘
           │ IPC              │ IPC
┌──────────▼──────┐  ┌───────▼───────────────┐
│ Renderer (动画)  │  │ Renderer (设置窗口)    │
│ index.html       │  │ settings.html          │
│ - 猫咪动画播放   │  │ - 参数配置界面          │
│ - 倒计时显示     │  │ - 保存/取消操作         │
│ - 用户交互       │  │                        │
└─────────────────┘  └────────────────────────┘
           │
┌──────────▼──────┐
│ Preload Bridge   │
│ preload.js       │
│ - contextBridge  │
│ - 安全 IPC 暴露  │
└─────────────────┘
```

### 2.2 安全模型

- `contextIsolation: true` — 渲染进程与 Node.js 环境完全隔离
- `nodeIntegration: false` — 渲染进程无法直接访问 Node.js API
- 通过 `contextBridge` 仅暴露必要的 API 接口

### 2.3 单实例锁

应用启动时通过 `app.requestSingleInstanceLock()` 确保同一时间只有一个实例运行。若已有实例在运行，新启动进程会在入口处直接 `app.exit(0)`，不会创建窗口、托盘或启动监控逻辑。

## 3. 文件结构

```
purr-pause/
├── main.js                  # 主进程入口
├── preload.js               # 预加载脚本（IPC 桥接）
├── config.json              # 默认配置文件
├── package.json             # 项目配置与构建脚本
├── lib/
│   ├── license.js           # 序列号验证与激活管理
│   ├── logger.js            # 日志记录
│   └── message-notify.js    # 消息提醒（小火箭）轮询与通知
├── tools/
│   ├── keygen.js            # 序列号生成器（开发者用）
│   ├── uninstall-linux.sh   # Linux 卸载脚本
│   ├── uninstall-mac.sh     # macOS 卸载脚本
│   └── uninstall-windows.ps1 # Windows 卸载脚本
├── renderer/
│   ├── index.html           # 猫咪动画渲染页面
│   ├── settings.html        # 设置界面
│   ├── activation.html      # 激活窗口
│   ├── rules.html           # 计时规则说明窗口
│   ├── message-list.html    # 待办消息列表窗口
│   ├── message-settings.html # 消息提醒设置窗口
│   └── rocket-demo.html     # 小火箭消息提醒动画窗口
├── assets/
│   ├── webm/                # 内置视频素材
│   │   ├── cat-walk.webm    # 猫走路动画
│   │   └── cat-rest.webm    # 猫休息动画
│   └── images/
│       ├── logo.png         # 应用 logo 源图
│       └── tray-icon.png    # 系统托盘图标
├── build/
│   └── icons/               # 应用图标（多平台）
└── docs/                    # 文档目录
```

## 4. 核心模块

### 4.1 空闲检测 (Idle Detection)

#### 检测方法优先级

应用启动时通过 `detectIdleMethod()` 自动检测可用的空闲时间获取方式：

**Linux:**
1. **xprintidle** (优先) — X11 环境下的轻量级工具
2. **gdbus (GNOME Mutter)** — 通过 D-Bus 调用 GNOME 桌面的 IdleMonitor
3. **降级处理** — 若两者均不可用，空闲时间始终返回 0

**macOS:**
- **ioreg** — 通过 `ioreg -c IOHIDSystem` 读取 `HIDIdleTime`（系统内置，返回纳秒精度，转换为毫秒）

**Windows:**
- **PowerShell + GetLastInputInfo** — 通过 Win32 API `GetLastInputInfo` 获取自上次输入以来的毫秒数

#### 监控逻辑

```javascript
// 每 10 秒检测一次
monitorInterval = setInterval(() => {
  getIdleTimeMs((idleMs) => {
    // 空闲超过 10 分钟 → 重置计时器（视为已休息）
    if (idleMs > 10 * 60 * 1000) {
      activeSeconds = 0;
      return;
    }
    // 空闲 < 3 分钟 → 活跃，累加 10 秒
    // 3~10 分钟 → 不累加也不归零（暂停）
    // 达到阈值 → 触发猫咪动画
  });
}, 10000);
```

#### 活跃时间计算规则

每 10 秒执行一次检测，按以下优先级判断：

| 优先级 | 条件 | 行为 |
|--------|------|------|
| 1 | 动画正在显示 | 跳过，不检测 |
| 2 | 用户手动暂停监控 | 跳过，不检测 |
| 3 | 屏幕已锁定 | 跳过，不检测；解锁时若锁屏超过 10 分钟则计时归零 |
| 4 | 系统休眠后唤醒 | 休眠超过 10 分钟则计时归零；短时休眠继续累计 |
| 5 | 距上次鼠标/键盘操作 > 10 分钟 | 计时归零（视为已休息） |
| 6 | 距上次鼠标/键盘操作 < 3 分钟 | 计时 +10 秒（视为活跃） |
| 7 | 距上次鼠标/键盘操作 3~10 分钟 | 不累加也不归零（暂停） |
| 8 | 累计活跃时间 ≥ 设定阈值 | 触发猫咪动画 |

#### 锁屏检测

**Linux:**
- 通过 D-Bus 监听 `org.gnome.ScreenSaver.ActiveChanged` 信号
- 锁屏 → 立即停止计时（不累加）
- 解锁 → 若锁屏超过 10 分钟则计时归零，否则恢复检测并由 `xprintidle` 决定后续行为
- gdbus 进程异常退出 → 重置为未锁屏状态，5 秒后自动重连

**macOS:**
- 优先使用 Electron `powerMonitor` 的 `lock-screen` / `unlock-screen` 事件
- 每 5 秒轮询 `Quartz.CGSessionCopyCurrentDictionary` 检查 `CGSSessionScreenIsLocked`
- 降级方案：直接解析 `ioreg -n Root -d1 -a` 输出中的 `CGSSessionScreenLockedTime`

**Windows:**
- 优先使用 Electron `powerMonitor` 的 `lock-screen` / `unlock-screen` 事件
- 每 5 秒轮询检测 `LogonUI` 进程是否存在（锁屏时该进程运行）

#### 系统休眠/唤醒

应用通过 Electron `powerMonitor` 监听 `suspend` / `resume` 事件。休眠前会清理挂起中的空闲检测；唤醒后会重新探测空闲检测方式，并重新确认当前锁屏状态。休眠超过 10 分钟时，按“已休息”处理并将计时归零；短时间休眠则保留休眠前累计时间并继续计时。

macOS 下原先的 `ioreg | grep -c CGSSessionScreenLockedTime` 降级检测在未锁屏时会因 `grep` 返回非 0 状态而跳过解锁更新，可能导致 `isScreenLocked` 长期停留在 `true`。当前实现改为直接解析 `ioreg` 输出，未找到锁屏字段时会明确恢复为未锁屏。

### 4.2 视频动画系统

#### 动画模式

应用支持四种动画模式，通过配置项 `animationMode` 控制：

| 模式 | 值 | 说明 |
|------|------|------|
| 右下角走出 | `walk-zoom` | 从右下角走入，边走边放大到屏幕中央 |
| 中央淡入 | `fade-center` | 在屏幕中央由小变大淡入，最终按屏幕宽高自适应 |
| 右侧走出 | `walk-flat` | 以固定大小从右侧走到屏幕中央，不缩放 |
| 中央走出 | `walk-center` | 在屏幕中央播放走路动画，由小到大走出（默认） |

动画模式切换为激活用户专属功能。

#### 动画流程（walk-zoom 模式）

```
触发 → 创建全屏透明窗口 → 加载 walk 视频 → 播放走路动画
  → walk 视频结束 → 切换到 idle 视频（循环） → 显示倒计时覆盖层
  → 倒计时结束或用户点击 → 淡出动画 → 关闭窗口 → 重置计时器
```

#### 动画流程（fade-center 模式）

```
触发 → 创建全屏透明窗口 → 加载 idle 视频
  → 在屏幕中央从 scale(0.05) 放大到 scale(1)，同时淡入（3 秒）
  → 显示倒计时覆盖层
  → 倒计时结束或用户点击 → 淡出动画 → 关闭窗口 → 重置计时器
```

#### 动画流程（walk-flat 模式）

```
触发 → 创建全屏透明窗口 → 加载 walk 视频 → 以固定大小从右侧走入
  → walk 视频结束 → 切换到 idle 视频（循环） → 显示倒计时覆盖层
  → 倒计时结束或用户点击 → 淡出动画 → 关闭窗口 → 重置计时器
```

#### 动画流程（walk-center 模式）

```
触发 → 创建全屏透明窗口 → 加载 walk 视频 → 在屏幕中央由小到大走出
  → walk 视频结束 → 切换到 idle 视频（循环） → 显示倒计时覆盖层
  → 倒计时结束或用户点击 → 淡出动画 → 关闭窗口 → 重置计时器
```

#### 走路动画 (walkZoom / walkFlat / walkCenter)

- `walk-zoom` 起始位置：屏幕右侧外偏下 (`right: -220px, bottom: 48px`)
- `walk-flat` 起始位置：屏幕右侧外，垂直居中
- `walk-center` 起始位置：屏幕中央
- 目标位置：屏幕中央
- `walk-zoom` 缩放：从 0.4 逐渐放大至同时满足屏幕高度 85% 与屏幕宽度 90% 的比例（最大 5x）
- `walk-flat` 缩放：固定为同时满足屏幕高度 85% 与屏幕宽度 90% 的比例（最大 5x）
- `walk-center` 缩放：固定在屏幕中央，从 0.05 逐渐放大至同时满足屏幕高度 85% 与屏幕宽度 90% 的比例（最大 5x），同时淡入
- 缓动函数：`1 - Math.pow(1 - progress, 3)` (ease-out cubic)
- 动画时长：与 walk 视频时长同步

#### 空闲动画 (showIdle)

- 无缝衔接 walk 动画的最终位置和尺寸
- 循环播放 idle 视频
- 800ms 后显示操作覆盖层

#### 消失动画 (dismiss)

- 800ms 淡出效果
- 清理视频资源释放内存

### 4.3 配置管理

#### 配置文件优先级

1. 用户配置：`~/.config/purr-pause/config.json`
2. 默认配置：应用内置 `config.json`

应用显示名为“胖猫暂停一下”，但主进程显式将 `userData` 固定到 appData 下的 `purr-pause` 目录，避免品牌显示名变化导致配置、日志和授权文件迁移。

#### 配置项

| 字段 | 类型 | 默认值 | 范围 | 说明 |
|------|------|--------|------|------|
| `thresholdMinutes` | number | 45 | 1-480 | 连续使用多久后触发提醒 |
| `breakMinutes` | number | 5 | 1-60 | 休息倒计时时长 |
| `animationMode` | string | `walk-center` | `walk-zoom` / `fade-center` / `walk-flat` / `walk-center` | 动画模式（仅激活用户） |
| `walkVideo` | string | `cat-walk.webm` | — | 走路动画文件名 |
| `idleVideo` | string | `cat-rest.webm` | — | 休息动画文件名 |
| `customWebmDir` | string | `""` | — | 自定义素材目录路径（仅激活用户） |
| `messageNotifyEnabled` | boolean | `false` | — | 是否启用消息提醒（小火箭） |
| `messageApiBaseUrl` | string | `""` | — | 消息接口基础地址（兼容旧配置） |
| `messageAuthHeaders` | string | `"{}"` | — | 消息接口请求头 JSON（兼容旧配置） |
| `messageSources` | array | `[]` | — | 消息接口源列表（名称/地址/请求头/启用） |
| `messagePollInterval` | number | 60 | 10-3600 | 消息轮询间隔（秒） |
| `messageMaxCacheItems` | number | 30 | 1-30 | 消息本地缓存条数上限 |
| `messageNotifyDir` | string | `""` | — | 小火箭素材目录 |
| `messageNotifyVideo` | string | `notify-rocket.webm` | — | 小火箭动画文件名 |
| `messageNotifyAnimation` | string | `rocket-corner` | — | 小火箭动画模式 |

#### 配置保存

配置通过 IPC 从设置窗口传递到主进程，经过范围校验后写入用户配置目录。

### 4.4 系统托盘集成

#### 托盘功能

- 显示应用图标（22x22 像素）
- Tooltip 显示 "胖猫暂停一下（PurrPause）"
- 菜单显示：
  - 激活状态（试用中/已激活/未激活）
  - 距下次休息剩余时间（仅分钟数变化时更新菜单，避免闪动）
  - 待办消息（未读数）
  - 消息提醒设置
  - 暂停监控（子菜单：30 分钟 / 1 小时 / 2 小时 / 直到手动恢复）
  - 激活/续期
  - 设置
  - 计时规则（独立窗口展示计时判断逻辑）
  - 查看日志（打开日志文件）
  - 小火箭演示
  - 立即测试
  - 重置计时
  - 版本号
  - 退出

#### 托盘图标路径

- 开发模式：`./assets/images/tray-icon.png`
- 打包模式：`{resourcesPath}/assets/images/tray-icon.png`

## 5. IPC 通信流

### 5.1 通信通道

| 通道名 | 方向 | 用途 |
|--------|------|------|
| `start-animation` | Main → Renderer | 触发猫咪动画，传递配置和视频路径 |
| `overlay-shown` | Renderer → Main | 覆盖层显示完成，启用鼠标事件 |
| `dismissed` | Renderer → Main | 用户关闭了提醒 |
| `snooze` | Renderer → Main | 用户点击"再等 5 分钟" |
| `load-config` | Main → Settings | 向设置窗口发送当前配置（含 `_isActivated`、`_autoLaunch`） |
| `save-config` | Settings → Main | 保存新配置 |
| `close-settings` | Settings → Main | 关闭设置窗口 |
| `pick-webm-dir` | Settings → Main | 请求选择素材目录 |
| `webm-dir-picked` | Main → Settings | 返回选择的目录路径 |
| `activate` | Activation → Main | 提交序列号激活 |
| `skip-activation` | Activation → Main | 跳过激活 |
| `license-status` | Main → Activation | 发送许可证状态和机器码 |
| `activation-result` | Main → Activation | 返回激活结果 |
| `save-config-result` | Main → Settings | 返回配置保存结果 |
| `download-message-api-spec` | Settings → Main | 请求下载消息提醒接口规范 |
| `download-message-api-spec-result` | Main → Settings | 返回接口规范下载结果 |
| `refresh-messages` | Renderer → Main | 手动刷新消息列表 |
| `open-message-target` | Renderer → Main | 打开指定消息的目标链接 |
| `dismiss-message-notification` | Renderer → Main | 关闭小火箭消息通知 |
| `test-message-connection` / `test-message-connection-result` | 双向 | 测试消息接口连通性 |
| `messages-state` | Main → Renderer | 推送消息列表状态 |

### 5.2 Preload 暴露的 API

```javascript
window.electronAPI = {
  onStartAnimation(callback)  // 监听动画启动
  overlayShown()              // 通知覆盖层已显示
  dismiss()                   // 通知已关闭
  snooze()                    // 请求延后 5 分钟
  onLoadConfig(callback)      // 监听配置加载（含 _isActivated、_autoLaunch 标志）
  saveConfig(config)          // 发送保存配置
  closeSettings()             // 请求关闭设置窗口
  pickWebmDir()               // 请求选择素材目录
  onWebmDirPicked(callback)   // 监听目录选择结果
  activate(serial)            // 发送激活请求
  skipActivation()            // 跳过激活
  onActivationResult(callback) // 监听激活结果
  onLicenseStatus(callback)   // 监听许可证状态（含 machineId）
  onSaveConfigResult(callback) // 监听配置保存结果
  downloadMessageApiSpec()    // 请求下载消息提醒接口规范
  onDownloadMessageApiSpecResult(callback) // 监听接口规范下载结果
  onMessagesState(callback)   // 监听消息列表状态
  refreshMessages()           // 手动刷新消息
  openMessageTarget(id)       // 打开消息目标链接
  dismissMessageNotification() // 关闭小火箭消息通知
  testMessageConnection(config) // 测试消息接口连通性
  onTestMessageConnectionResult(callback) // 监听测试结果
}
```

## 6. 多显示器处理

- 使用 `screen.getAllDisplays()` 获取所有显示器
- 动画窗口在每个显示器上各创建一个，精确覆盖各自的工作区域 (`workArea`)
- 窗口位置使用显示器的绝对坐标 (`x, y, width, height`)
- 设置窗口居中于主显示器工作区
- 关闭时销毁所有显示器上的窗口

## 7. 性能优化

### 7.1 GPU 加速

```javascript
app.commandLine.appendSwitch('enable-gpu-rasterization');
app.commandLine.appendSwitch('enable-zero-copy');
app.commandLine.appendSwitch('disable-software-rasterizer');
```

### 7.2 渲染优化

- `will-change: transform` / `will-change: opacity` — 提示浏览器创建独立合成层
- `contain: layout style` — 限制走路动画容器的布局和样式计算范围
- `requestAnimationFrame` — 使用浏览器原生帧调度
- `backgroundThrottling: false` — 防止后台节流影响动画

### 7.3 内存管理

- 动画窗口每次触发时销毁重建（解决 GNOME 工作区切换问题）
- 视频仅在需要时加载
- 窗口关闭后自动释放所有渲染资源

### 7.4 异步空闲检测

- 使用 `execFile`（非阻塞）替代 `execFileSync` 进行运行时检测
- 启动时使用 `execFileSync` 仅做一次方法探测

## 8. 视频素材要求

### 8.1 格式规范

| 属性 | 要求 |
|------|------|
| 容器格式 | WebM |
| 视频编码 | VP9 |
| Alpha 通道 | 必须（实现透明背景） |
| 建议尺寸 | 200~500px |
| 背景 | 透明 |

### 8.2 素材文件

| 文件 | 用途 | 播放方式 |
|------|------|---------|
| `cat-walk.webm` | 猫走路进入画面 | 播放一次 |
| `cat-rest.webm` | 猫躺下休息 | 循环播放 |

### 8.3 素材查找顺序

1. 自定义目录：用户在设置中指定的路径（仅激活用户可用）
2. 用户目录：`~/.config/purr-pause/webm/`
3. 内置目录：`{app}/assets/webm/`

## 9. 序列号与激活系统

### 9.1 机器码

机器码由以下系统信息 SHA256 哈希后取前 16 位十六进制字符生成：

```javascript
const parts = [os.platform(), os.arch(), cpuModel];
// Linux 追加 /etc/machine-id
// macOS 追加 IOPlatformUUID
// Windows 追加 MachineGuid
// 以上均不可用时追加 os.hostname() + os.homedir()
machineId = sha256(parts.join('|')).substring(0, 16);
```

### 9.2 序列号编码

序列号使用 Ed25519 非对称签名。内部二进制结构（76 字节）：

| 偏移 | 长度 | 内容 |
|------|------|------|
| 0 | 4 字节 | 有效天数（UInt32BE，0 = 永久） |
| 4 | 8 字节 | 机器码前 8 字节 |
| 12 | 64 字节 | Ed25519 签名 |

格式化为 `PP-` 前缀，后接 Base64 编码数据，并按每 8 个字符用 `-` 分隔。

### 9.3 激活校验

1. 解析序列号，使用内置 Ed25519 公钥验证签名
2. 提取嵌入的机器码，与当前机器码比对
3. 检查该序列号哈希是否已在本机使用过
4. 匹配且未使用过则写入 license.json 并签名

### 9.4 防篡改

- license.json 带 `_sig` 字段（HMAC 签名），篡改后校验失败
- `.install_mark` 文件记录安装日期并签名，防止删除 license.json 重置试用期
- 辅助标记文件存储在 `~/.local/share/.purr-pause-mark`（Linux/macOS）或 `%LOCALAPPDATA%\.purr-pause-mark`（Windows）
- license.json 中的 `machineId` 字段防止跨机器复制

### 9.5 功能限制

| 功能 | 试用 | 过期 | 已激活 |
|------|------|------|--------|
| 休息提醒 | ✓ | ✗ | ✓ |
| 时间设置 | ✓ | ✗ | ✓ |
| 动画模式切换 | ✗ | ✗ | ✓ |
| 自定义素材目录 | ✗ | ✗ | ✓ |

限制在两层实施：
- 前端：设置窗口中按钮置灰（`_isActivated` 标志）
- 后端：`save-config` 处理器校验许可证状态后才保存 `animationMode` 和 `customWebmDir`
- 过期状态：启动时弹出激活窗口，不启动计时器；关闭激活窗口会退出应用

## 10. 构建系统

### 10.1 electron-builder 配置

构建工具使用 `electron-builder`（v26+），支持多平台打包：

| 平台 | 目标格式 | 命令 |
|------|---------|------|
| Linux | .deb / AppImage（x64 / arm64） | `npm run build:linux:all` |
| macOS | .dmg（Intel / Apple Silicon） | `npm run build:mac -- --x64 --arm64` |
| Windows | .exe NSIS（x64 / arm64） | `npm run build:win:all` |
| 全平台 | 全部 | `npm run build:all` |

打包身份使用 ASCII slug `purr-pause`，因此 Linux deb 安装到 `/opt/purr-pause`，可执行文件为 `/opt/purr-pause/purr-pause`；桌面菜单显示名通过 Linux desktop entry 覆盖为“胖猫暂停一下”。Linux deb 包通过 `deb.afterInstall` 执行 `scripts/postinstall.sh`。使用 `sudo dpkg -i` 安装时，脚本会先把 `/opt/purr-pause/chrome-sandbox` 设置为 `root:root` 和 `4755`，满足 Electron SUID sandbox 要求；随后根据 `SUDO_USER` 为真实桌面用户写入 `~/.config/autostart/purr-pause.desktop`。应用内“开机自动启动”开关也会读写同一个用户自启动文件。macOS 和 Windows 继续使用 Electron 的 `setLoginItemSettings()`。

### 10.2 打包内容

```json
"productName": "purr-pause",
"executableName": "purr-pause",
"linux": {
  "target": "deb",
  "category": "Utility",
  "icon": "build/icons",
  "executableName": "purr-pause",
  "desktop": {
    "entry": {
      "Name": "胖猫暂停一下",
      "Comment": "胖猫暂停一下（PurrPause）- 一只胖猫提醒你适时休息",
      "StartupWMClass": "purr-pause"
    }
  }
},
"files": [
  "main.js",
  "preload.js",
  "lib/**/*",
  "renderer/**/*",
  "docs/message-api-spec.md",
  "config.json",
  "!tools"
],
"extraResources": [
  {
    "from": "assets",
    "to": "assets"
  }
],
"deb": {
  "depends": [],
  "compression": "gz",
  "afterInstall": "scripts/postinstall.sh"
}
```

### 10.3 额外资源

`assets` 目录作为 `extraResources` 打包，运行时通过 `process.resourcesPath` 访问。

## 11. 依赖

### 11.1 运行时依赖

无外部 npm 运行时依赖。应用仅依赖 Electron 内置模块：

- `electron` (app, BrowserWindow, Tray, Menu, ipcMain, nativeImage, screen, dialog, shell, powerMonitor)
- Node.js 内置模块：`path`, `fs`, `child_process`, `url`

### 11.2 开发依赖

| 包名 | 版本 | 用途 |
|------|------|------|
| electron | 33.2.1 | 应用运行时框架 |
| electron-builder | ^26.0.0 | 打包与分发工具 |

### 11.3 系统依赖

| 工具 | 用途 | 必要性 |
|------|------|--------|
| xprintidle | X11 空闲时间检测 | 推荐（优先使用） |
| gdbus | GNOME 空闲时间检测 | 备选方案 |

## 12. 开发模式

启动开发模式：

```bash
npm run dev
# 或
electron . --dev
```

开发模式下 `thresholdMinutes` 设为 0.15 分钟（9 秒），方便快速测试动画效果。
