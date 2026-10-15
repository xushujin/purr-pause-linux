# 胖猫暂停一下（PurrPause） 序列号激活系统 — 设计文档

## 概述

将胖猫暂停一下（PurrPause）做成共享软件模式：免费试用 7 天，到期后需要输入序列号激活。序列号绑定用户机器码，一机一码，不可在其他机器上使用。同一序列号会记录在本机授权文件中，避免在同一份授权记录里重复激活。序列号可以是限时的（例如 30 天 / 90 天 / 365 天）或永久的。开发者根据用户提供的机器码通过命令行工具生成序列号。

---

## 序列号格式

```
PP-AAAAHuUj-SdoLuryE-yw2o8C7N-YESaYoGw-TgnHEEFH-MacxJF+x-DNBEetfH-35EnA8WM-1/ZHGYai-XPYDVSO6-jJvu+bin-BpKoC9fr-LvONBw==
```

以 `PP-` 为前缀，后接 Base64 编码数据按每 8 字符用 `-` 分隔。

内部二进制结构（共 76 字节）：
- Payload（12 字节）：
  - 有效天数（4 字节 Big-Endian uint32，0 = 永久）
  - 机器码前 8 字节（从 16 位十六进制机器码解码）
- Ed25519 签名（64 字节）：对 Payload 的数字签名

### 签名算法

- **生成端**（keygen.js）：使用 Ed25519 私钥对 12 字节 Payload 签名
- **验证端**（license.js）：使用 Ed25519 公钥验证签名
- 非对称签名，即使反编译应用拿到公钥也无法伪造序列号

---

## 机器码

每台机器有唯一的 16 位十六进制机器码（SHA-256 哈希取前 16 字符），由以下信息生成：

- 操作系统平台 (`os.platform()`)
- CPU 架构 (`os.arch()`)
- CPU 型号 (`os.cpus()[0].model`)
- 系统唯一标识：
  - Linux: `/etc/machine-id`
  - macOS: `IOPlatformUUID`（通过 `ioreg` 获取）
  - Windows: `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`
  - 以上均不可用时回退到 `hostname + homedir`

用户在激活窗口中可以看到并复制自己的机器码，发送给开发者以获取对应的序列号。

---

## 文件结构

```
purr-pause/
├── tools/
│   └── keygen.js          # 命令行序列号生成器（开发者用）
├── lib/
│   └── license.js         # 序列号验证 + 试用期管理（主程序引用）
├── renderer/
│   └── activation.html    # 激活窗口 UI
└── main.js                # 启动时检查激活状态
```

---

## 激活流程

### 状态机

```
首次安装 → 试用中(7天) → 试用到期 → 用户提供机器码 → 开发者生成序列号 → 用户输入序列号 → 已激活
                                                                                          ↓
                                                                                    激活到期 → 续期
```

### 详细流程

1. **首次启动** — 记录安装日期到 `~/.config/purr-pause/license.json`，进入 7 天试用
2. **试用期内** — 正常使用，托盘菜单显示"试用中（剩余 X 天）"
3. **试用到期** — 弹出激活窗口，显示机器码，必须输入序列号才能继续使用
4. **用户获取序列号** — 用户复制机器码发送给开发者，开发者用 keygen 生成绑定该机器的序列号
5. **激活成功** — 写入 license.json，根据序列号类型设置到期时间或永久
6. **已激活到期** — 限时序列号到期后，再次弹出激活窗口

---

## license.json 结构

```json
{
  "installDate": "2026-05-12T00:00:00Z",
  "serialKey": "PP-AAAAHuUj-...",
  "activatedAt": "2026-05-12T00:00:00Z",
  "expiresAt": "2027-05-12T00:00:00Z",
  "type": "yearly",
  "machineId": "e52349da0bbabc84",
  "usedSerials": ["sha256hash1...", "sha256hash2..."],
  "_sig": "hmac签名..."
}
```

type 取值：`"trial"` | `"monthly"` | `"quarterly"` | `"yearly"` | `"permanent"`

expiresAt 为 `null` 时表示永久授权。

`usedSerials` 数组记录本机授权文件中已使用过的序列号 SHA-256 哈希（前 32 字符），防止同一份授权记录中重复激活同一个序列号。

`_sig` 字段为 HMAC-SHA256 签名，对 license.json 中除 `_sig` 外的关键字段计算，防止手动篡改。

---

## 命令行生成器用法

```bash
# 生成限时序列号（需要用户的机器码）
node tools/keygen.js --mid a1b2c3d4e5f67890 --days 30       # 30 天
node tools/keygen.js --mid a1b2c3d4e5f67890 --days 90       # 90 天
node tools/keygen.js --mid a1b2c3d4e5f67890 --days 365      # 365 天

# 生成永久序列号
node tools/keygen.js --mid a1b2c3d4e5f67890 --permanent

# 验证序列号
node tools/keygen.js --verify <序列号>
```

---

## 激活窗口 UI

橘色主题（与设置窗口一致），宽度 540px，窗口高度会根据内容和屏幕高度自适应：

- 标题："激活 胖猫暂停一下（PurrPause）"
- 授权状态提示（"试用已到期" / "试用中（剩余 X 天）" / "已激活"）
- 机器码显示区域（monospace 字体 + 复制按钮）
- 序列号输入框（textarea，支持直接粘贴完整序列号）
- 激活按钮（序列号长度 >= 20 字符时启用）
- "稍后再说"按钮（试用期内或已激活时可见，点击关闭激活窗口；过期状态不可跳过）
- 状态消息区域（成功绿色 / 错误红色，带淡入动画）

---

## 主程序改动 (main.js)

- 启动时调用 `license.checkStatus()` 获取状态
- 状态为 `expired` → 弹出激活窗口，阻止正常功能（不启动计时器）
- 状态为 `trial` → 正常运行，托盘显示剩余天数
- 状态为 `active` → 正常运行，托盘显示激活状态
- 托盘菜单加"激活/续期"选项

---

## 功能限制

| 功能 | 试用用户 | 激活用户 |
|------|---------|---------|
| 基本休息提醒 | ✓ | ✓ |
| 自定义时间设置 | ✓ | ✓ |
| 动画模式切换 | ✗ | ✓ |
| 自定义素材目录 | ✗ | ✓ |

---

## 安全考虑

- 序列号使用 Ed25519 非对称签名，应用内只有公钥，即使反编译也无法伪造序列号
- 序列号绑定机器码，一机一码，不可在其他机器上使用
- 同一序列号不能在同一份本机授权记录中重复使用（`usedSerials` 数组记录已用序列号哈希；无服务端全局防重）
- license.json 带 HMAC-SHA256 签名（`_sig` 字段），篡改后失效
- `.install_mark` 文件记录安装日期并签名，防止删除 license.json 重置试用期
- 辅助标记文件存储在 `~/.local/share/.purr-pause-mark`（Linux/macOS）或 `%LOCALAPPDATA%\.purr-pause-mark`（Windows），双重防护
- 机器指纹绑定，license.json 复制到其他机器无效
- 不做联网验证（完全离线可用）
- 序列号解析时自动 trim 空白字符，容错用户粘贴时带入的换行/空格

---

## 验证步骤

1. 启动应用，确认托盘菜单显示"试用中"，再通过"激活/续期"打开激活窗口并确认显示机器码
2. 复制机器码，用 `node tools/keygen.js --mid <机器码> --days 30` 生成序列号
3. 输入生成的序列号，确认激活成功
4. 用另一台机器的机器码生成序列号，在本机输入，确认提示"序列号与本机不匹配"
5. 手动修改 license.json 的 installDate 为 8 天前，重启应用，确认弹出激活窗口
6. 确认托盘菜单显示激活状态和到期时间
7. `node tools/keygen.js --mid <机器码> --permanent` 生成永久序列号，验证永久激活
