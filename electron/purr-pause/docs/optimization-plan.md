# 胖猫暂停一下（PurrPause） 优化计划

## 已完成

- [x] 暂停监控（演示/会议模式）— 托盘菜单支持暂停 30 分钟 / 1 小时 / 2 小时 / 手动恢复
- [x] 延后休息（"再等 5 分钟"）— 每次最多延后 2 次，延后后 5 分钟再次触发
- [x] 开机自启 — Ubuntu 使用 `~/.config/autostart/purr-pause.desktop`，macOS/Windows 使用 `app.setLoginItemSettings()`
- [x] 清理 license.js 死代码 — 删除 PLACEHOLDER 注释和未使用的 `formatSerial()`
- [x] 多显示器支持 — 猫咪同时出现在所有显示器
- [x] 托盘菜单显示剩余时间 — 仅分钟数变化时更新，避免闪动
- [x] 监控周期优化 — 从 30 秒缩短到 10 秒，提高计时精度

---

## 待实施

### P1 - 产品完整度

### 4. idle 检测不可用时提示

**问题：** 当系统没有 `xprintidle` 也没有 GNOME gdbus 时，`idleMethod = null`，程序永远认为用户活跃。用户离开电脑后回来会发现猫已经跳出来了，体验困惑。

**方案：**
- 启动时如果 `idleMethod === null`：
  - 首次启动弹出通知（Electron Notification API）："建议安装 xprintidle 以获得更准确的休息检测"
- 降级为纯计时模式（当前行为不变），但用户知道这个限制
- 在设置页面底部显示当前 idle 检测方式

**涉及文件：** `main.js`、`renderer/settings.html`

---

### P2 - 代码质量

### 6. 托盘菜单性能优化

**问题：** 每次 rebuildTrayMenu 都调用 `license.checkStatus()`，涉及文件读取 + JSON 解析 + HMAC 计算。

**方案：**
- 将 license 状态缓存在内存变量中
- 只在以下时机更新缓存：
  - 应用启动时
  - 激活成功时
  - 每天首次打开托盘菜单时（处理到期）
- `rebuildTrayMenu()` 从缓存读取，不再每次都做 I/O

**涉及文件：** `main.js`

---

### 7. activate IPC 频率限制

**问题：** 渲染进程可以高频调用 activate IPC，每次触发文件读写和签名验证。

**方案：**
- 在 `ipcMain.on('activate')` 处理器中加节流：1 秒内最多处理 1 次
- 超频调用直接返回 `{ success: false, error: '操作过于频繁' }`

**涉及文件：** `main.js`

---

### 8. usedSerials 数组上限

**问题：** 每次激活都往数组追加哈希，理论上无限增长。

**方案：**
- 保留最近 50 个已用序列号哈希
- 超出时移除最早的（FIFO）
- 正常使用场景下不会超过几个，纯防御性措施

**涉及文件：** `lib/license.js`
