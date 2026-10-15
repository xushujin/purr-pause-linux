# 消息提醒/待办通知模块设计

> 状态：MVP 已实现，并于 2026-05 重构为「单 list 接口」方案。本文档作为内部设计与迭代约束；第三方对接以 `docs/message-api-spec.md` 为准。
>
> 2026-05 更新：去掉 count 接口，直接轮询 list；每源待办固定上限 30（可配置，默认 30）；点击待办打开 `targetUrl` 后本地清理该条；角标取本地保留条数（≤30），不再取自服务端 total/unread。
>
> 2026-05 更新（响应精简）：待办项响应去掉 `source`、`status`、`updatedAt` 三个字段；客户端不再按状态过滤（第三方只返回未完成待办即可），排序改为按 `createdAt` 倒序；来源标签改为取自用户在 PurrPause 配置的源（响应不再携带来源信息）。
>
> 2026-05 更新（接口地址）：源配置由「服务地址(Base URL) + 写死路径 `/purr-pause/v1/todos`」改为「用户填**完整列表接口 URL**（`listUrl`）」，应用只在其后追加 `limit`/`offset`；`/purr-pause/v1/todos` 降级为**推荐约定路径**，第三方可挂任意路径、不再被强制挂主机根路径。旧 `messageApiBaseUrl` / 源 `baseUrl` 自动迁移为 `listUrl`（与约定路径拼接，保持老行为）。

## 当前结论

- 消息提醒是可选模块，默认关闭，客户可在设置中开启或关闭。
- 第三方系统按 PurrPause 定义的接口规范开发，不再让用户写 JS 表达式适配任意响应。
- 第三方只需提供一个 HTTP JSON 接口：待办消息列表（已去掉早期的“待办总数 count”接口）。
- 应用直接按固定间隔轮询列表接口（limit ≤ 30、offset = 0），用返回项的 id 与本地已知集合比对来发现新待办，降低接口数量和接入成本。
- 新消息提醒包含两部分：消息数量角标、轻量动画提醒。
- 提醒动画应尽量不遮挡屏幕、不抢焦点、不影响鼠标键盘操作；推荐使用右下角或屏幕边缘的透明点击穿透窗口。
- 应用内提供“待办消息列表”窗口；点击消息项打开第三方提供的 `targetUrl`，并在本地清理该条（角标 -1、列表移除），不回写第三方。
- 待办列表需要按来源做明显区分：来源标签和颜色取自用户在 PurrPause 中为每个接口配置的源（名称 + 内置色板），客户端据此渲染来源标签。
- 客户端每源只保留最近 30 条（可配置，默认 30）消息快照；超过上限时丢弃旧消息，避免列表和内存无限增长。
- 角标取本地保留的待办条数（每源 ≤ 30 之和），不取自服务端 total/unread。

## 当前实现状态

- 已新增 `lib/message-notify.js`，负责轮询、差异检测、角标状态、轻量动画、待办列表窗口生命周期和退避。
- 已新增 `renderer/message-list.html`，用于展示待办列表、刷新状态、来源标签和跳转入口。
- 已复用 `renderer/rocket-demo.html` 作为轻量消息提醒动画窗口。
- 已在 `main.js`、`preload.js`、`renderer/message-settings.html`、`config.json` 中接入默认配置、独立消息提醒设置窗口、IPC、托盘入口、保存校验和退出清理。
- 第一版仍不做服务端推送、已读回写、本地持久化完整消息内容和系统通知中心集成。

## 背景和目标

PurrPause 当前主要提供休息提醒。新模块用于连接客户已有的第三方业务系统，比如工单、审批、告警、待办任务系统。应用不保存业务数据，只做“轮询、提醒、展示列表、跳转第三方网页”。

目标是让接入方式稳定、可测试、可交付：

- 我们定义固定接口契约。
- 第三方按契约返回数据。
- PurrPause 只依赖标准字段，不执行用户自定义提取脚本。
- 后续可以给第三方一份独立接口文档和联调清单。

## MVP 范围

### 做

- 独立“消息提醒设置”窗口提供“消息提醒”总开关。
- 独立“消息提醒设置”窗口配置一个或多个第三方接口源（完整列表接口 URL、认证请求头、启用开关）和全局轮询间隔、每源待办条数。
- 按固定间隔轮询“待办消息列表”接口（limit ≤ 30、offset = 0）。
- 用返回项 id 与本地已知集合比对，检测到新增待办时显示数量角标，并播放一次提醒动画。
- 提供待办消息列表窗口。
- 点击消息项用系统默认浏览器打开第三方 `targetUrl`，并在本地清理该条（角标 -1、列表移除）。
- 关闭开关后停止轮询、隐藏角标、关闭提醒动画窗口，但保留配置。

### 暂不做

- 不做服务端推送、WebSocket、SSE。
- 不做消息已读回写接口（点击只在本地清理，不回写第三方）。
- 不在本地持久化完整消息内容（含本地清理记录，均仅存内存）。
- 不做复杂消息详情页；详情由第三方网站承载。
- 不做系统通知中心集成。

## 第三方接口规范 v1

### 通用要求

- 协议：HTTPS 推荐，开发环境可用 HTTP。
- 数据格式：请求和响应均为 JSON。
- 字符编码：UTF-8。
- 时间格式：ISO 8601，例如 `2026-05-26T10:30:00+08:00`。
- 鉴权：由客户配置请求头，推荐 `Authorization: Bearer <token>`。
- 所有接口应在 5 秒内返回；超时由客户端按失败处理。
- 第三方必须保证同一条待办消息的 `id` 稳定不变。

### 接口：获取待办消息列表

PurrPause 只调用这一个接口，既用于展示列表，也用于轮询检测新待办。该接口应轻量。

接口地址由用户在 PurrPause 填写**完整 URL**（第三方可用任意路径，推荐 `/purr-pause/v1/todos`）；PurrPause 只在其后追加 `limit`/`offset`，URL 原有的协议/主机/路径与其余查询参数保持不变。

```http
GET https://oa.example.com/purr-pause/v1/todos?limit=30&offset=0
Authorization: Bearer <token>
Accept: application/json
```

请求约定：

| 参数 | 类型 | 说明 |
|------|------|------|
| `limit` | number | PurrPause 固定以 `limit ≤ 30`（=「每源待办条数」，1–30，默认 30）轮询，只取最近 N 条 |
| `offset` | number | PurrPause 固定传 `0`，不翻页 |

第三方需按 `createdAt` 倒序返回，保证截断后保留的是最新待办。

成功响应：

```json
{
  "code": 0,
  "message": "ok",
  "data": {
    "total": 12,
    "items": [
      {
        "id": "todo-10086",
        "title": "审批单待处理",
        "summary": "张三提交了采购审批，需要你处理",
        "level": "normal",
        "createdAt": "2026-05-26T09:30:00+08:00",
        "targetUrl": "https://example.com/todos/todo-10086"
      }
    ]
  }
}
```

消息项字段：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `id` | string | 是 | 全局稳定唯一 ID |
| `title` | string | 是 | 列表主标题 |
| `summary` | string | 否 | 简短描述，建议 120 字以内 |
| `level` | string | 否 | `low` / `normal` / `high` / `urgent` |
| `createdAt` | string | 否 | 创建时间，用于排序；缺失时按接口返回顺序 |
| `targetUrl` | string | 是 | 点击后打开的第三方页面 URL |

客户端不再按状态过滤，会如实展示收到的每一条；第三方应只返回未完成的待办（已完成 / 已取消的不要返回）。

来源标签说明：

- 列表里每条待办的来源标签和颜色，来自用户在 PurrPause 中为该接口配置的源名称，响应本身无需携带来源信息。
- 点击某条打开 `targetUrl` 后本地清理该条，主进程推回新状态、列表即时移除。

### 点击与本地清理

- 点击待办打开其 `targetUrl`（系统默认浏览器）。
- 打开成功后 PurrPause **仅本地移除该条**（角标 -1、列表移除），不回写第三方，也不需要第三方提供“已读 / 完成”接口。
- 后续轮询第三方可继续返回该条：PurrPause 记住本地已清理的 id，不重复提醒、不重复弹动画；第三方按自身业务不再返回时，PurrPause 自然遗忘清理记录。
- 清理记录仅存内存，重启后已点击但服务端仍在返回的待办会重新出现，但首次轮询只建基线、不弹动画。

### 错误响应

```json
{
  "code": 40101,
  "message": "invalid token",
  "data": null
}
```

建议状态码：

| HTTP 状态 | 说明 |
|-----------|------|
| 200 | 业务请求成功，具体看 `code` |
| 400 | 请求参数错误 |
| 401 | 认证失败 |
| 403 | 无权限 |
| 429 | 请求过于频繁 |
| 500 | 第三方服务内部错误 |

客户端处理策略：

- 非 2xx 或 `code !== 0` 视为失败。
- 失败只写日志和设置页状态提示，不弹打扰性提醒。
- 连续失败进入退避。

## 客户端配置

`config.json` 和 `DEFAULT_CONFIG` 新增字段建议：

```json
{
  "messageNotifyEnabled": false,
  "messageApiBaseUrl": "",
  "messageAuthHeaders": "{}",
  "messageSources": [],
  "messagePollInterval": 60,
  "messageMaxCacheItems": 30,
  "messageNotifyDir": "",
  "messageNotifyVideo": "notify-rocket.webm",
  "messageNotifyAnimation": "rocket-corner"
}
```

| 字段 | 类型 | 默认值 | 校验 |
|------|------|--------|------|
| `messageNotifyEnabled` | boolean | `false` | 总开关 |
| `messageSources` | array | `[]` | 接口源数组，每项 `{ id, name, listUrl, authHeaders, enabled }`；`listUrl` 是**完整列表接口 URL**，启用源的 `listUrl` 必须是 `http:`/`https:` |
| `messageApiBaseUrl` | string | `""` | 旧单源字段，仅用于迁移：与约定路径 `/purr-pause/v1/todos` 拼成 `listUrl` 后写入 `messageSources` |
| `messageAuthHeaders` | string | `"{}"` | 旧单源字段，保留仅用于迁移 |
| `messagePollInterval` | number | `60` | 10-3600 秒，所有源共用 |
| `messageMaxCacheItems` | number | `30` | 1-30，每源查询并保留的最近待办条数（同时驱动列表显示与角标封顶） |
| `messageNotifyDir` | string | `""` | 小火箭素材目录；保存后自动生成 `purr-pause-消息提醒素材说明.txt`；为空时依次查找通用素材目录、用户默认 webm 目录和内置目录 |
| `messageNotifyVideo` | string | `notify-rocket.webm` | 文件名，不允许路径分隔符 |
| `messageNotifyAnimation` | string | `rocket-corner` | 预设动画模式 |

命名上建议使用 `message*`，因为这是对外产品能力；如果实现时沿用旧草案的 `taskPoll*`，需要在文档中明确映射，避免后续配置混乱。

## 用户体验设计

### 消息提醒设置窗口

托盘菜单提供独立入口：

- `消息提醒设置`

点击后打开独立设置窗口，不混入休息提醒的通用设置窗口。

- `启用消息提醒`：总开关。
- `待办接口地址`：完整的待办列表接口 URL，例如 `https://oa.example.com/purr-pause/v1/todos`；应用只在其后追加 `limit`/`offset`。
- `接口规范`：下载独立 Markdown 文件 `PurrPause-消息提醒接口规范-v1.md`，只包含第三方接口规范。
- `请求头`：JSON textarea，例如 `{"Authorization":"Bearer xxx"}`。
- `轮询间隔`：秒，默认 60。
- `小火箭素材目录`：可选目录，放置 `notify-rocket.webm`。
- 选择小火箭素材目录并保存后，客户端在目录下生成 `purr-pause-消息提醒素材说明.txt`。
- `提醒动画素材`：视频文件名或素材选择。
- `测试连接`：可选按钮，用于调用待办列表接口并展示返回条数。

关闭开关时：

- 不启动轮询。
- 不创建动画窗口。
- 不显示角标。
- 不清空用户已填写的配置。

### 角标设计

角标应该稳定可见但不干扰：

- 托盘菜单显示 `待办消息：N`。
- 设置页或待办列表入口显示数量。
- 如果未来有 Dock/任务栏 badge 支持，可按平台补充。

不建议把角标做成长期悬浮在桌面上的窗口；这会增加遮挡和误触风险。

### 提醒动画设计

用户提出的小火箭效果可以实现，推荐方式：

- 创建一个小尺寸透明无边框窗口，宽高约 `180x180` 到 `260x260`。
- 窗口位置从主屏幕右下角或左下角开始，沿屏幕边缘向上移动。
- 使用 `setIgnoreMouseEvents(true, { forward: true })`，让鼠标事件穿透，不影响工作。
- `focusable: false`、`skipTaskbar: true`，不抢焦点、不出现在任务栏。
- 动画只播放一次，约 7 秒后自动销毁窗口。
- 动画窗口只覆盖小区域，不创建全屏覆盖层。

小火箭示例流程：

1. 新待办到来，计算新增数量。
2. 若当前没有休息提醒覆盖层，也没有正在播放的消息动画，创建透明动画窗口。
3. 加载用户提供的 `notify-rocket.webm`。
4. 窗口从屏幕角落移动到顶部附近。
5. 播放完成后自动关闭。
6. 角标 = 本地保留的待办条数（每源 ≤ 30、已扣除点击清理项），每次轮询按最新快照重算；点击清理一条立即 -1，服务端无待办时归零。

如果新消息连续到来：

- 动画播放中不重复创建第二个火箭。
- 只更新待办角标。
- 动画结束后，如果仍有新的 pending 提醒，可按冷却时间决定是否再播放一次。

### 待办消息列表

应用提供一个“待办消息”窗口，入口建议放在托盘菜单：

- `待办消息（N）`
- 点击打开列表窗口。

列表窗口内容：

- 顶部显示总数、最后刷新时间、刷新按钮。
- 列表项显示标题、摘要、来源、等级、创建时间。
- 来源以彩色标签展示，标签名称和颜色取自用户配置的源（颜色由源 id 哈希到内置调色板，稳定分配）。
- 不同第三方来源的标签位置和样式保持一致，避免用户只靠颜色辨认。
- 点击列表项调用 `shell.openExternal(item.targetUrl)` 打开第三方网站。
- URL 必须是 `http:` 或 `https:`，非法 URL 不打开并记录日志。

列表刷新策略：

- 打开窗口时立即拉取列表。
- 点击刷新按钮时拉取列表。
- 后台轮询刷新后，如果窗口已打开，自动推送新状态并重渲染。
- 点击某条打开 `targetUrl` 后本地清理该条，主进程推回新状态、列表即时移除。

列表性能策略：

- 列表渲染合并后的全部条目（每源已封顶 `messageMaxCacheItems` ≤ 30）。
- 本地内存快照每源最多保留 `messageMaxCacheItems` 条（默认 30）。
- 超过上限时按 `createdAt` 倒序保留最新消息，丢弃旧消息。
- 如果第三方返回的 `total` 大于本地保留条数，列表顶部提示“仅显示最近 N 条”。
- 角标取本地保留条数（每源 ≤ 30 之和、已扣除点击清理项），不取自第三方 `total`/`unread`。
- 列表项中的长标题和摘要必须截断，避免单条消息导致布局抖动。

## 模块边界

```
lib/message-notify.js        # 轮询、差异检测、角标状态、通知生命周期
renderer/message-list.html   # 待办消息列表窗口
renderer/message-settings.html # 消息提醒独立设置窗口
renderer/rocket-demo.html    # 小火箭/轻量提醒动画窗口（复用）
preload.js                   # 增加消息列表和通知 IPC
main.js                      # 生命周期、托盘入口、配置集成
renderer/settings.html       # 通用休息提醒设置 UI
config.json                  # 默认配置
assets/webm/notify-rocket.webm # 可选内置示例素材
```

### lib/message-notify.js

工厂函数 `createMessageNotify(config, deps)` 初始化模块（必要时启动轮询），返回以下控制方法：

- `updateConfig(nextConfig)`：更新配置，开关或关键字段变化时重启。
- `start()`：立即拉取一次列表（建立基线），然后开启定时器。
- `stop()`：停止轮询，关闭提醒动画，保留配置。
- `destroy()`：停止轮询，关闭所有消息相关窗口。
- `refreshList()`：拉取待办列表。
- `openMessageList()`：打开或聚焦待办列表窗口。
- `openMessageTarget(id)`：打开某条消息的 `targetUrl`，打开成功后本地清理该条。
- `dismissNotification()`：关闭当前小火箭动画窗口。
- `handleRestOverlayChanged(showing)`：休息覆盖层显隐变化通知（结束后补播挂起的提醒）。
- `getBadgeCount()`：返回当前聚合角标数。
- `getState()`：返回当前聚合待办状态快照。

> `createMessageNotify.testConnection(formConfig, logger)` 另以静态方法导出，供设置页「测试连接」调用。

依赖由 `main.js` 注入：

- `BrowserWindow`
- `screen`
- `shell`
- `preloadPath`
- `rendererDir`
- `logger`
- `getNotifyMediaUrl(filename)`
- `isRestOverlayShowing()`
- `onBadgeChange(count)`：用于刷新托盘菜单。

## 轮询和差异检测

1. 启用后按间隔直接 GET 用户配置的 `listUrl`，并在其上追加 `limit=<N≤30>&offset=0`（推荐约定路径为 `/purr-pause/v1/todos`）。
2. 列表数据进入内存前先标准化：
   - 过滤掉缺失 `id`/`title` 或 `targetUrl` 非法的项。
   - 按 `createdAt` 倒序排序，取前 `messageMaxCacheItems`（≤30）条。
   - 注入来源标签 `origin`（取自用户配置的源 id/name + 内置色板）和唯一键 `uid`。
3. 把本地「已点击清理」的 id 集合（`clearedIds`）裁剪为「本次返回里仍存在的 id」，再用它过滤掉被清理的条目，得到 `snapshot`。
4. 用 `snapshot` 中的 `id` 与本地 `knownIds` 比较，计算新增消息；随后把 `snapshot` 的 id 记入 `knownIds`。
5. 角标设为 `snapshot.length`（每源 ≤ 30），聚合角标为各启用源之和。
6. **首次同步**只建立基线（填充 `snapshot`/`knownIds`、设角标），不播放动画。
7. 非首次同步且新增数 > 0 时：
   - 播放一次提醒动画。
   - 列表窗口已打开时自动收到新状态并重渲染。
8. 列表为空（服务端无待办）时 `snapshot` 自然清空、角标归零。

注意：角标取本地保留条数（已扣除点击清理项），不取自服务端 `total`/`unread`；提醒动画只反映“有新消息到来”。点击一条待办后本地角标立即 -1。

## 状态机

| 状态 | 说明 | 进入条件 | 退出条件 |
|------|------|----------|----------|
| `disabled` | 功能关闭 | 默认或用户关闭开关 | 用户开启且配置有效 |
| `idle` | 已启用，等待轮询 | 初始化完成 | 定时器触发 |
| `polling-list` | 正在拉取列表 | 启动、定时器触发、用户打开列表或手动刷新 | 成功或失败 |
| `notifying` | 正在播放轻量动画 | 检测到新增消息 | 动画结束 |
| `backoff` | 连续失败后降频 | 连续失败达到阈值 | 下一次成功 |

## 失败与退避

- 列表轮询失败按源记录。
- 连续失败 5 次后该源轮询间隔翻倍，最大 300 秒。
- 任意一次成功后失败计数归零，恢复用户配置间隔。
- 失败不播放动画、不弹错误通知。
- 设置页和待办列表窗口可以显示最后错误，例如“最近同步失败：认证失败”。

## 来源标注和颜色

待办列表不能只靠文字来源区分。每条消息展示一个来源标签，标签信息来自用户在 PurrPause 中配置的源（`messageSources`），响应不携带来源字段：

- 标签文案使用配置源的 `name`。
- 标签颜色按配置源的 `id` 哈希到内置色板，保证同一来源颜色稳定。
- 标签同时使用文字和颜色，不只依赖颜色，照顾色弱用户。
- `level` 仍用于表示紧急程度，不能和来源颜色混用。

建议内置色板使用区分度较高、不过度刺眼的颜色，例如：

| 色值 | 用途 |
|------|------|
| `#2f80ed` | 蓝色来源 |
| `#27ae60` | 绿色来源 |
| `#f2994a` | 橙色来源 |
| `#9b51e0` | 紫色来源 |
| `#eb5757` | 红色来源 |
| `#00a7a7` | 青色来源 |

如果后续支持客户配置来源映射，可以增加：

```json
{
  "messageSourceStyles": {
    "oa": { "name": "OA", "color": "#2f80ed" },
    "jira": { "name": "Jira", "color": "#27ae60" }
  }
}
```

第一版不要求设置页编辑这份映射，先以用户配置的源名称和内置色板为主（颜色按源 id 哈希到内置色板）。

## 性能和消息淘汰

消息提醒模块必须控制内存、渲染和网络成本：

- 只轮询 list 接口；同一源同一时刻只允许一个请求在飞（手动刷新与定时轮询互斥，跳过本轮并重排）。
- list 请求使用 `limit=messageMaxCacheItems&offset=0`，单次最多拉取 30 条，要求第三方按 `createdAt`（创建时间）倒序返回。
- 客户端每源最多保留 `messageMaxCacheItems` 条消息，默认 30。
- 超过上限时丢弃旧消息，不写入磁盘。
- `knownIds` 有上限（默认保留最近 2000 个 id）；`clearedIds`（点击清理记录）每轮裁剪为「本次返回仍存在的 id」，天然有界（≤ 本次条数），另设安全上限避免轮询间隔内无限堆积。
- 列表窗口渲染合并后的全部条目（每源已封顶 30）。
- 如果服务端 `total > 本地保留条数`，列表顶部提示“仅显示最近 N 条”；角标仍取本地保留条数，不取服务端 `total`。

淘汰规则：

1. 按 `createdAt` 倒序。
2. `createdAt` 缺失时按本次接口返回顺序。
3. 保留前 `messageMaxCacheItems` 条。
4. 丢弃消息只影响本地展示，不影响第三方系统真实待办。

## 安全与隐私

- 只允许打开 `http:` / `https:` 的 `targetUrl`。
- 不执行第三方返回的脚本或 HTML。
- 列表窗口用 `textContent` 渲染标题和摘要，避免 HTML 注入。
- 请求头可能包含 token，日志中不能打印完整请求头。
- 不在本地持久化业务消息正文；内存中保留最近一次列表即可。
- 如果后续要持久化，需要加密或明确数据保留策略。

## 和休息提醒的关系

- 休息提醒优先级更高。
- `isOverlayShowing === true` 时不播放小火箭动画。
- 休息提醒期间仍可继续轮询和更新角标。
- 休息提醒结束后，如果期间有新增消息，可以播放一次合并后的提醒动画。
- 消息动画窗口不使用全屏覆盖，不影响当前休息提醒窗口的生命周期。

## IPC 设计

`preload.js` 新增：

- `onMessagesState(callback)`：列表窗口接收聚合后的待办状态（含 `items`、`badgeCount`、`hiddenCount`、`sources` 等）。
- `refreshMessages()`：列表窗口请求刷新。
- `openMessageTarget(id)`：列表窗口请求打开第三方页面（主进程打开成功后本地清理该条）。
- `dismissMessageNotification()`：关闭小火箭动画窗口。
- `testMessageConnection(config)` / `onTestMessageConnectionResult(callback)`：设置页测试连接（只测 list）。
- `downloadMessageApiSpec()` / `onDownloadMessageApiSpecResult(callback)`：下载第三方接口规范。

> 小火箭动画窗口不走消息 IPC：参数（label、count、mediaUrl、时长）通过 `loadFile` 的 query string 注入；角标变化经主进程 `onBadgeChange` 回调刷新托盘，并随 `messages-state` 推送到列表窗口。

`main.js` 新增：

- `ipcMain.on('dismiss-message-notification', ...)`
- `ipcMain.on('refresh-messages', ...)`
- `ipcMain.on('open-message-target', ...)`

## main.js 集成点

1. 引入 `lib/message-notify.js`。
2. `DEFAULT_CONFIG` 添加消息提醒配置字段。
3. `app.whenReady` 中初始化消息模块。
4. 托盘菜单增加 `待办消息（N）`，点击打开消息列表。
5. 托盘菜单增加 `消息提醒设置`，点击打开独立消息提醒设置窗口。
6. `save-config` IPC 校验并保存消息提醒配置，然后调用 `messageNotify.updateConfig(config)`。
7. `before-quit` 调用 `messageNotify.destroy()`。
8. 消息模块通过回调触发 `rebuildTrayMenu()` 更新角标。

## 实施拆分

### Phase 1：接口规范和配置 UI

- 固化本文档中的第三方接口规范。
- 更新默认配置字段。
- 新建独立消息提醒设置窗口。
- 支持开启/关闭、完整列表接口 URL、请求头、轮询间隔、动画素材。

验收：配置可保存；关闭开关时不启动轮询。

### Phase 2：轮询和角标

- 新建 `lib/message-notify.js`。
- 实现 list 接口轮询。
- 实现基于返回项 id 与本地 `knownIds` 的新增差异检测。
- 实现来源标签注入（取自配置源）、内置色板和每源 30 条淘汰上限。
- 托盘菜单显示 `待办消息（N）`。

验收：本地 mock 服务返回不同数量时，角标正确更新。

### Phase 3：待办消息列表

- 新建 `renderer/message-list.html`。
- 实现列表展示、手动刷新、最后刷新时间、错误状态。
- 实现来源标签和颜色区分。
- 实现列表渲染上限，超过上限时提示“仅显示最近 N 条”。
- 点击消息项打开 `targetUrl`。

验收：列表可打开，点击消息能跳转第三方网页。

### Phase 4：轻量动画提醒

- 复用 `renderer/rocket-demo.html`。
- 实现小火箭素材播放和窗口位移动画。
- 设置窗口为点击穿透、不抢焦点、不占任务栏。
- 新增消息时播放一次，连续新增时合并提醒。

验收：动画从屏幕角落到顶部后消失，不影响鼠标点击当前工作窗口。

### Phase 5：冲突处理和稳定性

- 休息提醒显示期间不播放消息动画。
- 连续失败退避。
- 退出应用时清理窗口和定时器。
- 日志脱敏。

验收：休息提醒、暂停监控、激活窗口、消息提醒互不破坏。

## 验证清单

1. 默认关闭消息提醒，不发起任何第三方请求。
2. 开启后配置无效 URL，保存失败或提示错误。
3. 配置 mock 第三方接口后，客户端**只**按间隔调用 list 接口（`/todos`），完全不请求 `/todos/count`。
4. 首次同步建立基线、不播放动画；列表能看到条目，角标 = 条数（≤30）。
5. mock 新增一条新 id：下次轮询播放一次小火箭、角标 +1、列表出现新条目。
6. 点击一条：浏览器打开其 `targetUrl`，该条从列表消失、角标 -1；下次轮询即使 mock 仍返回它也不再出现、不再弹动画。
7. mock 返回 >30 条：本地只保留 30、列表顶部提示“仅显示最近 30 条”、角标 = 30。
8. 设置页把「每源待办条数」改为如 5 并保存：查询/保留/显示都变 5。
9. 动画播放期间鼠标点击能穿透到后方窗口。
10. 托盘菜单显示 `待办消息（N）`，点击打开列表；`消息提醒设置` 打开独立设置窗口。
11. 「测试连接」只测 list，成功显示“列表返回 N 条”。
12. 配置多个源时，各源的待办显示各自的来源标签和稳定颜色（颜色由源 id 哈希到内置色板分配）。
13. 关闭消息提醒开关后停止轮询、关闭动画窗口、隐藏角标。
14. 第三方接口连续失败 5 次后进入退避（间隔翻倍），恢复后归位。
15. 休息提醒显示时不播放消息动画但角标仍更新；结束后合并补播一次。

## 已确认决策（2026-05 重构）

- 角标取本地保留的待办条数（每源 ≤ 30 之和），不再取服务端 `total`/`unread`。
- 去掉 count 接口，直接轮询 list。
- 每源固定上限 30，可配置（1–30，默认 30），同时驱动查询 limit、本地保留、列表显示、角标封顶。
- 点击待办打开 `targetUrl` 后本地清理该条，不回写第三方、不需要已读接口。
- 来源标签和颜色由用户在 PurrPause 配置的源决定（响应不携带 `source`），颜色按源 id 哈希到内置色板。
- 源配置改为用户填**完整列表接口 URL**（`listUrl`），应用只在其后追加 `limit`/`offset`；`/purr-pause/v1/todos` 为推荐约定路径，第三方可挂任意路径，不再强制挂主机根路径。旧 `messageApiBaseUrl`/源 `baseUrl` 自动迁移为 `listUrl`。

## 仍待确认

- 第三方 `targetUrl` 是否必须同域名；如果需要限制，应增加允许域名配置。
- 是否内置默认小火箭素材；如果不内置，需要在设置页明确提示用户提供素材。
- 消息提醒是否作为激活用户功能；如果是，需要在保存配置和 UI 上加授权限制。
- 每源 30 条对待办量很大的客户是否足够，是否后续做分页/虚拟列表。
