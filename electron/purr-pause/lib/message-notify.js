const path = require('path');

// 推荐约定路径；仅用于把旧「服务地址(origin)」配置迁移成完整 listUrl，不再写死进请求。
const DEFAULT_LIST_PATH = '/purr-pause/v1/todos';
const REQUEST_TIMEOUT_MS = 5000;
const KNOWN_ID_LIMIT = 2000;
const CLEARED_ID_LIMIT = 200;
const NOTIFY_SIZE = 240;
const NOTIFY_DURATION_MS = 7000;
const NOTIFY_LABEL = '待办消息';
const SOURCE_PALETTE = ['#2f80ed', '#27ae60', '#f2994a', '#9b51e0', '#eb5757', '#00a7a7'];

function clampNumber(value, min, max, fallback) {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.max(min, Math.min(max, parsed));
}

function normalizeGlobalConfig(config) {
  const cfg = config || {};
  return {
    messageNotifyEnabled: !!cfg.messageNotifyEnabled,
    messagePollInterval: clampNumber(cfg.messagePollInterval, 10, 3600, 60),
    messageMaxCacheItems: clampNumber(cfg.messageMaxCacheItems, 1, 30, 30),
    messageNotifyVideo: String(cfg.messageNotifyVideo || 'notify-rocket.webm').trim() || 'notify-rocket.webm',
    messageNotifyAnimation: cfg.messageNotifyAnimation === 'rocket-corner' ? 'rocket-corner' : 'rocket-corner'
  };
}

function resolveListUrl(src) {
  const direct = String(src && src.listUrl != null ? src.listUrl : '').trim();
  if (direct) return direct;
  // 旧配置迁移：把旧「服务地址(origin)」+ 约定路径拼成完整 listUrl，保持老行为。
  const legacyBase = String(
    src && src.baseUrl != null ? src.baseUrl
      : (src && src.messageApiBaseUrl != null ? src.messageApiBaseUrl : '')
  ).trim();
  if (!legacyBase) return '';
  try {
    return new URL(DEFAULT_LIST_PATH, legacyBase).toString();
  } catch (e) {
    return legacyBase;
  }
}

function normalizeSource(raw) {
  const src = raw || {};
  return {
    id: String(src.id || '').trim(),
    name: String(src.name || '').trim() || '未命名',
    listUrl: resolveListUrl(src),
    authHeaders: String(src.authHeaders != null ? src.authHeaders : (src.messageAuthHeaders != null ? src.messageAuthHeaders : '{}')).trim() || '{}',
    enabled: src.enabled === undefined ? true : !!src.enabled
  };
}

function extractSources(config) {
  const cfg = config || {};
  const rawList = Array.isArray(cfg.messageSources) ? cfg.messageSources : [];
  const result = [];
  const seen = new Set();
  for (const raw of rawList) {
    const source = normalizeSource(raw);
    let id = source.id;
    if (!id || seen.has(id)) {
      id = 'src-' + Math.random().toString(36).slice(2, 8).padEnd(6, '0');
      while (seen.has(id)) {
        id = 'src-' + Math.random().toString(36).slice(2, 8).padEnd(6, '0');
      }
    }
    source.id = id;
    seen.add(id);
    result.push(source);
  }
  return result;
}

function isHttpUrl(value) {
  try {
    const url = new URL(value);
    return url.protocol === 'http:' || url.protocol === 'https:';
  } catch (e) {
    return false;
  }
}

function isSafeTargetUrl(value) {
  return isHttpUrl(value);
}

function parseHeaders(value) {
  const parsed = JSON.parse(value || '{}');
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('请求头必须是 JSON 对象');
  }

  const headers = Object.create(null);
  for (const [key, rawValue] of Object.entries(parsed)) {
    const name = String(key || '').trim();
    if (!name) continue;
    if (rawValue === undefined || rawValue === null) continue;
    headers[name] = String(rawValue);
  }
  return headers;
}

function hashToPaletteIndex(value) {
  let hash = 0;
  const str = String(value || 'unknown');
  for (let i = 0; i < str.length; i++) {
    hash = ((hash << 5) - hash + str.charCodeAt(i)) | 0;
  }
  return Math.abs(hash) % SOURCE_PALETTE.length;
}

function safeColor(color, sourceId) {
  const value = String(color || '').trim();
  if (/^#[0-9a-fA-F]{6}$/.test(value)) return value.toLowerCase();
  return SOURCE_PALETTE[hashToPaletteIndex(sourceId)];
}

function toTimestamp(item) {
  const value = Date.parse(item.createdAt || '');
  return Number.isFinite(value) ? value : 0;
}

function normalizeItem(raw, order) {
  if (!raw || typeof raw !== 'object') return null;

  const id = String(raw.id || '').trim();
  const title = String(raw.title || '').trim();
  if (!id || !title) return null;

  const targetUrl = String(raw.targetUrl || '').trim();
  if (!isSafeTargetUrl(targetUrl)) return null;

  // 来源标签取自用户配置的源（见 decorateItem 注入的 origin），响应本身不携带 source。
  return {
    id,
    title,
    summary: raw.summary ? String(raw.summary) : '',
    level: raw.level ? String(raw.level) : 'normal',
    createdAt: raw.createdAt ? String(raw.createdAt) : '',
    targetUrl,
    _order: order
  };
}

function sanitizeHeadersForLog(headers) {
  if (!headers || typeof headers !== 'object') return '[]';
  return '[' + Object.keys(headers).join(',') + ']';
}

async function fetchWithDetails(url, headers) {
  if (typeof fetch !== 'function') {
    return { ok: false, error: '当前运行环境不支持 fetch' };
  }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const response = await fetch(url, { method: 'GET', headers, signal: controller.signal });
    const text = await response.text();
    let body = null;
    try {
      body = text ? JSON.parse(text) : null;
    } catch (e) {
      return { ok: false, error: '响应不是合法 JSON', status: response.status };
    }
    if (!response.ok) {
      const detail = body && body.message ? '：' + body.message : '';
      return { ok: false, error: 'HTTP ' + response.status + detail, status: response.status };
    }
    if (!body || body.code !== 0) {
      return { ok: false, error: (body && body.message) || '第三方接口返回失败 (code=' + (body && body.code) + ')', status: response.status };
    }
    return { ok: true, data: body.data || {}, status: response.status };
  } catch (e) {
    if (e && e.name === 'AbortError') return { ok: false, error: '请求超时（' + (REQUEST_TIMEOUT_MS / 1000) + ' 秒）' };
    return { ok: false, error: (e && e.message) ? e.message : String(e) };
  } finally {
    clearTimeout(timer);
  }
}

async function testConnection(rawSource, logger) {
  const log = (logger && typeof logger.info === 'function') ? logger : { info: () => {}, warn: () => {} };
  const source = normalizeSource(rawSource || {});
  const raw = rawSource || {};
  const maxCacheItems = clampNumber(
    raw.maxCacheItems != null ? raw.maxCacheItems : raw.messageMaxCacheItems,
    1,
    30,
    30
  );

  if (!isHttpUrl(source.listUrl)) {
    log.warn('[消息提醒][测试] 校验失败：接口地址不是 http/https URL');
    return { ok: false, error: '接口地址必须是 http 或 https URL' };
  }

  let headers;
  try {
    headers = parseHeaders(source.authHeaders);
  } catch (e) {
    log.warn('[消息提醒][测试] 校验失败：请求头解析失败 ' + (e && e.message ? e.message : String(e)));
    return { ok: false, error: '请求头解析失败：' + (e && e.message ? e.message : String(e)) };
  }

  const reqHeaders = { Accept: 'application/json', ...headers };
  const listUrl = new URL(source.listUrl);
  listUrl.searchParams.set('limit', String(maxCacheItems));
  listUrl.searchParams.set('offset', '0');
  log.info('[消息提醒][测试] 开始测试 listUrl=' + source.listUrl + ', headers=' + sanitizeHeadersForLog(headers));
  log.info('[消息提醒][测试] GET ' + listUrl.toString());
  const listResult = await fetchWithDetails(listUrl, reqHeaders);
  if (!listResult.ok) {
    log.warn('[消息提醒][测试] list 失败：' + listResult.error);
    return { ok: false, error: listResult.error };
  }
  const data = listResult.data || {};
  const itemCount = Array.isArray(data.items) ? data.items.length : 0;
  const total = Number.isFinite(data.total) ? data.total : itemCount;
  log.info('[消息提醒][测试] list 成功 items=' + itemCount + ', total=' + total);
  return { ok: true, total, itemCount };
}

function createMessageNotify(initialConfig, deps) {
  const shared = {
    config: normalizeGlobalConfig(initialConfig || {}),
    sources: new Map(),
    listWin: null,
    notifyWin: null,
    notifyTimer: null,
    pendingAnimationCount: 0,
    pendingAnimationLabel: NOTIFY_LABEL,
    pendingAnimationOrigin: null,
    destroyed: false
  };

  const logger = deps.logger;

  function logInfo(message) {
    if (logger && logger.info) logger.info(message);
  }

  function logWarn(message) {
    if (logger && logger.warn) logger.warn(message);
  }

  function safeParseHeaders(value) {
    try {
      return parseHeaders(value);
    } catch (e) {
      return {};
    }
  }

  // ---------------------------------------------------------------------------
  // 共享列表窗口 / 角标聚合
  // ---------------------------------------------------------------------------

  function aggregateBadgeCount() {
    let sum = 0;
    for (const runtime of shared.sources.values()) {
      if (runtime.sourceIsEnabled()) sum += runtime.badgeCount;
    }
    return sum;
  }

  let lastAggregatedBadge = 0;

  function recomputeBadge() {
    const next = aggregateBadgeCount();
    if (next === lastAggregatedBadge) return;
    logInfo('[消息提醒] 聚合角标变化: ' + lastAggregatedBadge + ' -> ' + next);
    lastAggregatedBadge = next;
    if (typeof deps.onBadgeChange === 'function') deps.onBadgeChange(next);
  }

  function onSourceBadgeChanged() {
    recomputeBadge();
    sendListState();
  }

  function coordinatorIsEnabled() {
    if (!shared.config.messageNotifyEnabled) return false;
    for (const runtime of shared.sources.values()) {
      if (runtime.source.enabled && isHttpUrl(runtime.source.listUrl)) return true;
    }
    return false;
  }

  function getPublicState() {
    const enabledRuntimes = [];
    for (const runtime of shared.sources.values()) {
      if (runtime.sourceIsEnabled()) enabledRuntimes.push(runtime);
    }

    let merged = [];
    let totalSum = 0;
    let hiddenCount = 0;
    let loading = false;
    let backoff = false;
    let lastRefreshAt = '';
    let lastError = '';

    for (const runtime of enabledRuntimes) {
      merged = merged.concat(runtime.snapshot);
      totalSum += runtime.total;
      // 仅当服务端待办超过每源保留上限（messageMaxCacheItems）才算「因封顶而隐藏」；
      // 本地点击清理不计入，避免清理后误触发「仅显示最近 N 条」提示。
      hiddenCount += Math.max(0, runtime.total - shared.config.messageMaxCacheItems);
      if (runtime.listInFlight) loading = true;
      if (runtime.consecutiveFailures >= 5) backoff = true;
      if (runtime.lastRefreshAt && (!lastRefreshAt || runtime.lastRefreshAt > lastRefreshAt)) {
        lastRefreshAt = runtime.lastRefreshAt;
      }
      if (!lastError && runtime.lastError) {
        lastError = runtime.source.name + ': ' + runtime.lastError;
      }
    }

    merged.sort((a, b) => {
      const timeDiff = toTimestamp(b) - toTimestamp(a);
      if (timeDiff) return timeDiff;
      return String(a.uid).localeCompare(String(b.uid));
    });

    const itemCount = merged.length;
    const total = Math.max(totalSum, itemCount);

    const sources = [];
    for (const runtime of shared.sources.values()) {
      const src = runtime.source;
      const enabled = !!shared.config.messageNotifyEnabled && src.enabled;
      sources.push({
        id: src.id,
        name: src.name,
        color: safeColor('', src.id),
        enabled,
        configured: isHttpUrl(src.listUrl),
        total: runtime.total,
        badgeCount: runtime.badgeCount,
        itemCount: runtime.snapshot.length,
        lastError: runtime.lastError,
        loading: runtime.listInFlight,
        backoff: runtime.consecutiveFailures >= 5,
        lastRefreshAt: runtime.lastRefreshAt
      });
    }

    return {
      enabled: !!shared.config.messageNotifyEnabled,
      configured: enabledRuntimes.length > 0,
      total,
      badgeCount: aggregateBadgeCount(),
      items: merged,
      itemCount,
      hiddenCount,
      maxCacheItems: shared.config.messageMaxCacheItems,
      pollInterval: shared.config.messagePollInterval,
      lastRefreshAt,
      lastError,
      loading,
      backoff,
      sources
    };
  }

  function sendListState() {
    if (!shared.listWin || shared.listWin.isDestroyed()) return;
    shared.listWin.webContents.send('messages-state', getPublicState());
  }

  // ---------------------------------------------------------------------------
  // 共享动画窗口（多源合并）
  // ---------------------------------------------------------------------------

  function closeNotification() {
    if (shared.notifyTimer) {
      clearTimeout(shared.notifyTimer);
      shared.notifyTimer = null;
    }
    if (shared.notifyWin && !shared.notifyWin.isDestroyed()) {
      const win = shared.notifyWin;
      shared.notifyWin = null;
      win.hide();
      setTimeout(() => {
        if (win && !win.isDestroyed()) win.destroy();
      }, 30);
      return;
    }
    shared.notifyWin = null;
  }

  function maybeReplayPendingNotification() {
    if (shared.pendingAnimationCount <= 0) return;
    if (typeof deps.isRestOverlayShowing === 'function' && deps.isRestOverlayShowing()) return;
    const count = shared.pendingAnimationCount;
    const label = shared.pendingAnimationLabel;
    shared.pendingAnimationCount = 0;
    shared.pendingAnimationLabel = NOTIFY_LABEL;
    shared.pendingAnimationOrigin = null;
    playNotification(count, label);
  }

  function accumulatePending(count, label) {
    // 合并挂起：若当前 pending 全来自同一源则保留该源名，否则置兜底 label。
    const labelText = label || NOTIFY_LABEL;
    if (shared.pendingAnimationCount <= 0) {
      shared.pendingAnimationOrigin = labelText;
      shared.pendingAnimationLabel = labelText;
    } else if (shared.pendingAnimationOrigin !== labelText) {
      shared.pendingAnimationOrigin = null;
      shared.pendingAnimationLabel = NOTIFY_LABEL;
    }
    shared.pendingAnimationCount += count;
  }

  function playNotification(count, label) {
    if (!coordinatorIsEnabled() || count <= 0) return;
    const labelText = label || NOTIFY_LABEL;
    if (typeof deps.isRestOverlayShowing === 'function' && deps.isRestOverlayShowing()) {
      accumulatePending(count, labelText);
      logInfo('[消息提醒] 跳过动画: 休息覆盖层显示中, 挂起 pending=' + shared.pendingAnimationCount);
      return;
    }
    if (shared.notifyWin && !shared.notifyWin.isDestroyed()) {
      accumulatePending(count, labelText);
      logInfo('[消息提醒] 跳过动画: 已有动画播放中, 挂起 pending=' + shared.pendingAnimationCount);
      return;
    }

    logInfo('[消息提醒] 播放消息动画, count=' + count + ', label=' + labelText);
    const display = deps.screen.getPrimaryDisplay();
    const area = display.workArea;
    const margin = 24;
    const startX = Math.round(Math.max(area.x + margin, area.x + area.width - NOTIFY_SIZE - margin));
    const startY = Math.round(Math.max(area.y + margin, area.y + area.height - NOTIFY_SIZE - margin));
    const endY = Math.round(Math.max(area.y + margin, area.y + area.height * 0.14));
    const localStartY = Math.max(0, startY - endY);
    const pathHeight = Math.max(NOTIFY_SIZE, localStartY + NOTIFY_SIZE);
    const mediaUrl = typeof deps.getNotifyMediaUrl === 'function'
      ? deps.getNotifyMediaUrl(shared.config.messageNotifyVideo)
      : '';

    shared.notifyWin = new deps.BrowserWindow({
      x: startX,
      y: endY,
      width: NOTIFY_SIZE,
      height: pathHeight,
      transparent: true,
      backgroundColor: '#00000000',
      frame: false,
      alwaysOnTop: true,
      skipTaskbar: true,
      focusable: false,
      hasShadow: false,
      show: false,
      paintWhenInitiallyHidden: true,
      resizable: false,
      webPreferences: {
        contextIsolation: true,
        nodeIntegration: false,
        backgroundThrottling: false
      }
    });

    const win = shared.notifyWin;
    win.setVisibleOnAllWorkspaces(true);
    win.setIgnoreMouseEvents(true, { forward: true });
    win.setBackgroundColor('#00000000');
    win.loadFile(path.join(deps.rendererDir, 'rocket-demo.html'), {
      query: {
        label: labelText,
        count: String(Math.max(1, count)),
        mediaUrl,
        startY: String(localStartY),
        endY: '0',
        durationMs: String(NOTIFY_DURATION_MS)
      }
    });

    win.webContents.on('did-finish-load', () => {
      if (!win || win.isDestroyed()) return;
      win.setAlwaysOnTop(true, 'screen-saver');
      win.setBackgroundColor('#00000000');
      win.webContents.executeJavaScript('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(resolve))))')
        .catch(() => {})
        .finally(() => {
          if (!win || win.isDestroyed()) return;
          win.showInactive();
          win.moveTop();
          shared.notifyTimer = setTimeout(() => {
            if (win && !win.isDestroyed()) closeNotification();
            setTimeout(maybeReplayPendingNotification, 1500);
          }, NOTIFY_DURATION_MS + 160);
        });
    });

    win.on('closed', () => {
      if (shared.notifyWin === win) shared.notifyWin = null;
      if (shared.notifyTimer) {
        clearTimeout(shared.notifyTimer);
        shared.notifyTimer = null;
      }
      setTimeout(maybeReplayPendingNotification, 1500);
    });
  }

  // ---------------------------------------------------------------------------
  // 每源 runtime（内部工厂）—— 与单源状态机行为等价
  // ---------------------------------------------------------------------------

  function createSourceRuntime(source) {
    const runtime = {
      source,
      config: shared.config,
      timer: null,
      listInFlight: false,
      firstSync: true,
      consecutiveFailures: 0,
      lastError: '',
      lastRefreshAt: '',
      total: 0,
      badgeCount: 0,
      snapshot: [],
      knownIds: new Set(),
      knownIdQueue: [],
      clearedIds: new Set(),
      clearedIdQueue: []
    };

    const tag = '[消息提醒][' + source.name + ']';

    function sourceIsEnabled() {
      return !!shared.config.messageNotifyEnabled && runtime.source.enabled && isHttpUrl(runtime.source.listUrl);
    }
    runtime.sourceIsEnabled = sourceIsEnabled;

    function buildListUrl() {
      const url = new URL(runtime.source.listUrl);
      url.searchParams.set('limit', String(shared.config.messageMaxCacheItems));
      url.searchParams.set('offset', '0');
      return url;
    }

    function getRequestHeaders() {
      return {
        Accept: 'application/json',
        ...parseHeaders(runtime.source.authHeaders)
      };
    }

    async function fetchJson(url) {
      if (typeof fetch !== 'function') {
        throw new Error('当前运行环境不支持 fetch');
      }

      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
      try {
        const response = await fetch(url, {
          method: 'GET',
          headers: getRequestHeaders(),
          signal: controller.signal
        });
        const text = await response.text();
        let body = null;
        try {
          body = text ? JSON.parse(text) : null;
        } catch (e) {
          throw new Error('响应不是合法 JSON');
        }

        if (!response.ok) {
          throw new Error('HTTP ' + response.status);
        }
        if (!body || body.code !== 0) {
          throw new Error((body && body.message) || '第三方接口返回失败');
        }
        return body.data || {};
      } finally {
        clearTimeout(timeout);
      }
    }

    function setBadgeCount(count) {
      const next = Math.max(0, Number.parseInt(count, 10) || 0);
      if (runtime.badgeCount === next) return;
      logInfo(tag + ' 角标变化: ' + runtime.badgeCount + ' -> ' + next);
      runtime.badgeCount = next;
      onSourceBadgeChanged();
    }

    function recordSuccess() {
      runtime.consecutiveFailures = 0;
      runtime.lastError = '';
    }

    function recordFailure(context, err) {
      runtime.consecutiveFailures += 1;
      const message = err && err.message ? err.message : String(err);
      runtime.lastError = context + '失败：' + message;
      logWarn(tag + ' ' + runtime.lastError + '（连续失败 ' + runtime.consecutiveFailures + ' 次）');
      sendListState();
    }

    function effectivePollIntervalMs() {
      const base = shared.config.messagePollInterval;
      if (runtime.consecutiveFailures < 5) return base * 1000;
      const backedOff = Math.min(300, base * Math.pow(2, runtime.consecutiveFailures - 4));
      return backedOff * 1000;
    }

    function clearPollTimer() {
      if (!runtime.timer) return;
      clearTimeout(runtime.timer);
      runtime.timer = null;
    }
    runtime.clearPollTimer = clearPollTimer;

    function scheduleNextPoll(delayMs) {
      clearPollTimer();
      if (shared.destroyed || !sourceIsEnabled()) return;
      runtime.timer = setTimeout(() => {
        pollList();
      }, delayMs);
      if (runtime.timer.unref) runtime.timer.unref();
    }
    runtime.scheduleNextPoll = scheduleNextPoll;

    function decorateItem(item) {
      const decorated = { ...item };
      delete decorated._order;
      decorated.origin = {
        id: runtime.source.id,
        name: runtime.source.name,
        color: safeColor('', runtime.source.id)
      };
      decorated.uid = runtime.source.id + '::' + decorated.id;
      return decorated;
    }

    async function fetchList() {
      const url = buildListUrl();
      const data = await fetchJson(url);
      const rawItems = Array.isArray(data.items) ? data.items : [];
      const items = rawItems
        .map((item, index) => normalizeItem(item, index))
        .filter(Boolean)
        .sort((a, b) => {
          const timeDiff = toTimestamp(b) - toTimestamp(a);
          return timeDiff || a._order - b._order;
        })
        .slice(0, shared.config.messageMaxCacheItems)
        .map((item) => decorateItem(item));

      return {
        total: Math.max(0, Number.parseInt(data.total, 10) || items.length),
        items
      };
    }

    function rememberKnownIds(items) {
      for (const item of items) {
        if (runtime.knownIds.has(item.id)) continue;
        runtime.knownIds.add(item.id);
        runtime.knownIdQueue.push(item.id);
      }

      while (runtime.knownIdQueue.length > KNOWN_ID_LIMIT) {
        const oldId = runtime.knownIdQueue.shift();
        runtime.knownIds.delete(oldId);
      }
    }

    function applyListData(data) {
      const fetched = Array.isArray(data.items) ? data.items : [];
      const fetchedIds = new Set(fetched.map((item) => item.id));

      // clearedIds 裁剪为「本次返回里仍存在的 id」：第三方已删/已完成的条目自然遗忘，集合有界 ≤ 本次条数。
      const keptQueue = [];
      const keptSet = new Set();
      for (const cid of runtime.clearedIdQueue) {
        if (fetchedIds.has(cid) && !keptSet.has(cid)) {
          keptQueue.push(cid);
          keptSet.add(cid);
        }
      }
      runtime.clearedIdQueue = keptQueue;
      runtime.clearedIds = keptSet;

      // 过滤掉已点击清理的条目，避免被清理项重新出现或重新触发动画。
      const snapshot = fetched.filter((item) => !runtime.clearedIds.has(item.id));
      const newItems = snapshot.filter((item) => !runtime.knownIds.has(item.id));

      runtime.snapshot = snapshot;
      runtime.total = data.total;
      runtime.lastRefreshAt = new Date().toISOString();
      rememberKnownIds(snapshot);
      setBadgeCount(snapshot.length);

      return { newCount: newItems.length, items: snapshot };
    }

    async function refreshList(options = {}) {
      if (!sourceIsEnabled()) {
        runtime.lastError = shared.config.messageNotifyEnabled ? '接口地址无效' : '';
        sendListState();
        return { newCount: 0, items: runtime.snapshot };
      }
      if (runtime.listInFlight) return { newCount: 0, items: runtime.snapshot };

      runtime.listInFlight = true;
      sendListState();
      try {
        const data = await fetchList();
        const result = applyListData(data);
        recordSuccess();
        logInfo(tag + ' 列表刷新成功: 服务端 total=' + data.total + ', 展示=' + result.items.length + ', 新增=' + result.newCount + (options.suppressNotification ? '（静默）' : ''));
        return {
          newCount: options.suppressNotification ? 0 : result.newCount,
          items: result.items
        };
      } catch (err) {
        recordFailure(options.manual ? '刷新待办列表' : '拉取待办列表', err);
        throw err;
      } finally {
        runtime.listInFlight = false;
        sendListState();
      }
    }
    runtime.refreshList = refreshList;

    async function pollList() {
      if (shared.destroyed || !sourceIsEnabled()) return;
      if (runtime.listInFlight) {
        // 已有刷新在进行（如手动刷新/打开列表），本轮跳过并重排，保持轮询不中断。
        scheduleNextPoll(effectivePollIntervalMs());
        return;
      }
      const isFirst = runtime.firstSync;
      const url = buildListUrl();
      logInfo(tag + ' 开始轮询 GET ' + url.toString());
      try {
        const result = await refreshList({ suppressNotification: isFirst });
        if (isFirst) {
          runtime.firstSync = false;
          logInfo(tag + ' 首次同步完成, 建立基线（不播放动画）');
        } else if (result.newCount > 0) {
          playNotification(result.newCount, runtime.source.name);
        } else {
          logInfo(tag + ' 列表已刷新但无新增, 不播放动画');
        }
      } catch (err) {
        // recordFailure 已在 refreshList 内记录。
      } finally {
        scheduleNextPoll(effectivePollIntervalMs());
      }
    }

    function clearMessage(id) {
      const targetId = String(id || '');
      if (!targetId) return false;
      const before = runtime.snapshot.length;
      runtime.snapshot = runtime.snapshot.filter((item) => item.id !== targetId);
      if (runtime.snapshot.length === before) return false;

      if (!runtime.clearedIds.has(targetId)) {
        runtime.clearedIds.add(targetId);
        runtime.clearedIdQueue.push(targetId);
        while (runtime.clearedIdQueue.length > CLEARED_ID_LIMIT) {
          const oldId = runtime.clearedIdQueue.shift();
          runtime.clearedIds.delete(oldId);
        }
      }

      runtime.lastRefreshAt = new Date().toISOString();
      logInfo(tag + ' 本地清理待办 id=' + targetId + ', 剩余 ' + runtime.snapshot.length + ' 条');
      // setBadgeCount 仅在数值变化时触发刷新；清理必然使快照 -1，会推送新列表状态与角标。
      setBadgeCount(runtime.snapshot.length);
      return true;
    }
    runtime.clearMessage = clearMessage;

    function resetRuntimeForStart() {
      clearPollTimer();
      runtime.firstSync = true;
      runtime.consecutiveFailures = 0;
      runtime.lastError = '';
    }

    function start(initialDelayMs) {
      if (!sourceIsEnabled()) return;
      resetRuntimeForStart();
      logInfo(tag + ' 轮询已启动, listUrl=' + runtime.source.listUrl + ', interval=' + shared.config.messagePollInterval + 's, headers=' + sanitizeHeadersForLog(safeParseHeaders(runtime.source.authHeaders)));
      scheduleNextPoll(Math.max(0, initialDelayMs || 0));
      sendListState();
    }
    runtime.start = start;

    function stop() {
      clearPollTimer();
      runtime.listInFlight = false;
      runtime.lastError = '';
      // 清空快照/total，使已停用源不再向 getPublicState 贡献残留条数（来源 chip 不再显示陈旧计数）。
      runtime.snapshot = [];
      runtime.total = 0;
      setBadgeCount(0);
      sendListState();
      logInfo(tag + ' 轮询已停止');
    }
    runtime.stop = stop;

    return runtime;
  }

  // ---------------------------------------------------------------------------
  // 协调器：start / stop / updateConfig（差异同步）
  // ---------------------------------------------------------------------------

  function start() {
    if (!coordinatorIsEnabled()) return;
    let index = 0;
    for (const runtime of shared.sources.values()) {
      if (!runtime.sourceIsEnabled()) continue;
      runtime.start(index * 300);
      index += 1;
    }
    recomputeBadge();
    sendListState();
  }

  function stop() {
    for (const runtime of shared.sources.values()) {
      runtime.stop();
    }
    closeNotification();
    shared.pendingAnimationCount = 0;
    shared.pendingAnimationLabel = NOTIFY_LABEL;
    shared.pendingAnimationOrigin = null;
    recomputeBadge();
    sendListState();
    logInfo('[消息提醒] 所有源轮询已停止');
  }

  function sourceConfigKey(source) {
    return JSON.stringify({
      listUrl: source.listUrl,
      authHeaders: source.authHeaders,
      enabled: source.enabled
    });
  }

  function updateConfig(nextConfig) {
    const prevGlobal = shared.config;
    shared.config = normalizeGlobalConfig(nextConfig || {});
    const nextSources = extractSources(nextConfig || {});

    const globalChanged = JSON.stringify(prevGlobal) !== JSON.stringify(shared.config);
    if (globalChanged) {
      logInfo('[消息提醒] 全局配置更新, enabled=' + shared.config.messageNotifyEnabled + ', interval=' + shared.config.messagePollInterval + 's, maxCache=' + shared.config.messageMaxCacheItems + ', 源数=' + nextSources.length);
    }

    // 全局关 → 全停清零，但保留 runtime 集合（差异同步仍按新配置整理）。
    if (!shared.config.messageNotifyEnabled) {
      stop();
    }

    const nextIds = new Set(nextSources.map((s) => s.id));

    // 删除：旧集合里不在新集合的源，stop + 清理。
    for (const [id, runtime] of Array.from(shared.sources.entries())) {
      if (nextIds.has(id)) continue;
      logInfo('[消息提醒][' + runtime.source.name + '] 源已删除, 停止并清理');
      runtime.stop();
      shared.sources.delete(id);
    }

    // 新增 / 更新：逐源比对。
    for (const source of nextSources) {
      const existing = shared.sources.get(source.id);
      if (!existing) {
        const runtime = createSourceRuntime(source);
        shared.sources.set(source.id, runtime);
        logInfo('[消息提醒][' + source.name + '] 新增源 runtime');
        if (shared.config.messageNotifyEnabled && runtime.sourceIsEnabled()) {
          runtime.start(0);
        }
        continue;
      }

      const sameConfig = sourceConfigKey(existing.source) === sourceConfigKey(source);
      // 始终更新展示信息（如名称），name 变化不影响轮询基线。
      const wasEnabled = existing.sourceIsEnabled();
      existing.source = source;

      if (!shared.config.messageNotifyEnabled) {
        // 全局关：上面已 stop，所有 runtime 已清零，无需再动。
        continue;
      }

      if (sameConfig) {
        // 同一源 listUrl/authHeaders/enabled 未变 → 不重置, 以免丢 firstSync 基线。
        if (existing.sourceIsEnabled() && !existing.timer) {
          existing.start(0);
        } else if (!existing.sourceIsEnabled() && (existing.timer || existing.badgeCount > 0)) {
          existing.stop();
        }
        continue;
      }

      // 配置变化 → 重置并按需重启。
      logInfo('[消息提醒][' + source.name + '] 源配置变化, 重置 runtime');
      if (existing.sourceIsEnabled()) {
        existing.start(0);
      } else if (wasEnabled || existing.timer || existing.badgeCount > 0) {
        existing.stop();
      }
    }

    recomputeBadge();
    sendListState();
  }

  // ---------------------------------------------------------------------------
  // 列表窗口 / 跳转 / 刷新 / 休息覆盖层
  // ---------------------------------------------------------------------------

  function refreshList(options = {}) {
    const tasks = [];
    for (const runtime of shared.sources.values()) {
      if (!runtime.sourceIsEnabled()) continue;
      tasks.push(runtime.refreshList(options).catch(() => ({ newCount: 0, items: runtime.snapshot })));
    }
    if (tasks.length === 0) {
      sendListState();
      return Promise.resolve({ newCount: 0, items: getPublicState().items });
    }
    return Promise.all(tasks).then((results) => {
      let newCount = 0;
      for (const result of results) {
        if (result && Number.isFinite(result.newCount)) newCount += result.newCount;
      }
      return { newCount, items: getPublicState().items };
    });
  }

  function openMessageList() {
    if (shared.listWin && !shared.listWin.isDestroyed()) {
      shared.listWin.focus();
      sendListState();
      if (coordinatorIsEnabled()) refreshList({ manual: true, suppressNotification: true }).catch(() => {});
      return;
    }

    const display = deps.screen.getPrimaryDisplay();
    const area = display.workArea;
    const width = 560;
    const height = Math.min(720, Math.round(area.height * 0.82));
    shared.listWin = new deps.BrowserWindow({
      x: Math.round(area.x + (area.width - width) / 2),
      y: Math.round(area.y + (area.height - height) / 2),
      width,
      height,
      minWidth: 460,
      minHeight: 420,
      show: false,
      frame: true,
      autoHideMenuBar: true,
      title: '待办消息',
      webPreferences: {
        preload: deps.preloadPath,
        contextIsolation: true,
        nodeIntegration: false
      }
    });

    const win = shared.listWin;
    win.setMenuBarVisibility(false);
    win.loadFile(path.join(deps.rendererDir, 'message-list.html'));
    win.webContents.on('did-finish-load', () => {
      sendListState();
      win.show();
      if (coordinatorIsEnabled()) refreshList({ manual: true, suppressNotification: true }).catch(() => {});
    });
    win.on('closed', () => {
      if (shared.listWin === win) shared.listWin = null;
    });
  }

  function openMessageTarget(idOrUid) {
    const key = String(idOrUid || '');
    let found = null;
    let owner = null;

    // 先按 uid 精确匹配。
    for (const runtime of shared.sources.values()) {
      const item = runtime.snapshot.find((entry) => entry.uid === key);
      if (item) {
        found = item;
        owner = runtime;
        break;
      }
    }

    // 否则按 id 匹配第一个。
    if (!found) {
      for (const runtime of shared.sources.values()) {
        const item = runtime.snapshot.find((entry) => entry.id === key);
        if (item) {
          found = item;
          owner = runtime;
          break;
        }
      }
    }

    if (!found) {
      logWarn('[消息提醒] 打开目标失败: 未找到消息 ' + key);
      return;
    }
    if (!isSafeTargetUrl(found.targetUrl)) {
      logWarn('[消息提醒] 打开目标失败: 非法 URL');
      return;
    }

    // 打开成功后本地清理该条（角标 -1、列表移除）；打开失败则保留，便于重试。
    const clearOpened = () => {
      if (owner && typeof owner.clearMessage === 'function') owner.clearMessage(found.id);
    };
    const result = deps.shell.openExternal(found.targetUrl);
    if (result && typeof result.then === 'function') {
      result.then(clearOpened, (err) => {
        logWarn('[消息提醒] 打开目标失败: ' + (err && err.message ? err.message : String(err)));
      });
    } else {
      clearOpened();
    }
  }

  function handleRestOverlayChanged(showing) {
    if (!showing) {
      setTimeout(maybeReplayPendingNotification, 500);
    }
  }

  function destroy() {
    shared.destroyed = true;
    for (const runtime of shared.sources.values()) {
      runtime.clearPollTimer();
    }
    closeNotification();
    if (shared.listWin && !shared.listWin.isDestroyed()) shared.listWin.destroy();
    shared.listWin = null;
  }

  // 构造：建立每源 runtime，若启用则 start（错峰）。
  for (const source of extractSources(initialConfig || {})) {
    shared.sources.set(source.id, createSourceRuntime(source));
  }
  if (coordinatorIsEnabled()) {
    start();
  } else {
    recomputeBadge();
  }

  return {
    updateConfig,
    start,
    stop,
    destroy,
    refreshList,
    openMessageList,
    openMessageTarget,
    dismissNotification: closeNotification,
    handleRestOverlayChanged,
    getBadgeCount: () => aggregateBadgeCount(),
    getState: getPublicState
  };
}

module.exports = createMessageNotify;
module.exports.testConnection = testConnection;
module.exports.resolveListUrl = resolveListUrl;
module.exports.DEFAULT_LIST_PATH = DEFAULT_LIST_PATH;
