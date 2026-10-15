const fs = require('fs');
const path = require('path');
const createMessageNotify = require('./message-notify');
const createAssets = require('./assets');

// 配置默认值与读写/迁移/校验。纯函数命名空间（无每实例状态）：
// config 对象由调用方（main.js）持有，本模块以参数接收并就地 mutate / 返回新对象，
// 不缓存其引用，避免跨模块状态快照失同步。
const DEFAULT_CONFIG = {
  thresholdMinutes: 45,
  breakMinutes: 5,
  customWebmDir: '',
  walkVideo: 'cat-walk.webm',
  idleVideo: 'cat-rest.webm',
  animationMode: 'walk-center',
  logEnabled: false,
  messageNotifyEnabled: false,
  messageApiBaseUrl: '',
  messageAuthHeaders: '{}',
  messageSources: [],
  messagePollInterval: 60,
  messageMaxCacheItems: 30,
  messageNotifyDir: '',
  messageNotifyVideo: 'notify-rocket.webm',
  messageNotifyAnimation: 'rocket-corner'
};

function clampNumber(value, min, max, fallback) {
  const parsed = parseInt(value, 10);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.max(min, Math.min(max, parsed));
}

function isHttpUrl(value) {
  try {
    const url = new URL(value);
    return url.protocol === 'http:' || url.protocol === 'https:';
  } catch (e) {
    return false;
  }
}

function normalizeMessageAuthHeaders(value) {
  const text = String(value || '{}').trim() || '{}';
  const parsed = JSON.parse(text);
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('请求头必须是 JSON 对象');
  }
  return JSON.stringify(parsed);
}

function genSourceId() {
  return 'src-' + Math.random().toString(36).slice(2, 8).padEnd(6, '0');
}

function migrateMessageSources(config) {
  if (!config || typeof config !== 'object') return;
  const sources = Array.isArray(config.messageSources) ? config.messageSources : [];
  // 旧单源迁移：仅当当前没有任何源且旧 baseUrl 合法时执行（幂等）
  if (sources.length === 0) {
    const legacyBaseUrl = String(config.messageApiBaseUrl || '').trim();
    if (isHttpUrl(legacyBaseUrl)) {
      config.messageSources = [{
        id: genSourceId(),
        name: '默认接口',
        listUrl: createMessageNotify.resolveListUrl({ baseUrl: legacyBaseUrl }),
        authHeaders: String(config.messageAuthHeaders || '{}').trim() || '{}',
        enabled: config.messageNotifyEnabled === true
      }];
      // 旧单源已迁移进 messageSources，清空旧字段，避免日后删除该源后下次启动又被复活。
      config.messageApiBaseUrl = '';
      config.messageAuthHeaders = '{}';
      return;
    }
    config.messageSources = [];
    return;
  }
  // 已有源：逐源补默认值/规整，并保证 id 非空且唯一
  const seenIds = new Set();
  config.messageSources = sources.map((raw) => {
    const source = raw && typeof raw === 'object' ? raw : {};
    let id = String(source.id || '').trim();
    if (!id || seenIds.has(id)) {
      id = genSourceId();
    }
    seenIds.add(id);
    return {
      id,
      name: String(source.name || '').trim() || '未命名',
      listUrl: createMessageNotify.resolveListUrl(source),
      authHeaders: String(source.authHeaders || '{}').trim() || '{}',
      enabled: !!source.enabled
    };
  });
}

// 校验并应用消息提醒相关配置到 config（就地 mutate）。
// generateNotifyReadme(dir) 为可选回调（由 main.js 注入 assets.generateMessageNotifyReadmeInDir），
// 使本模块不直接依赖 assets 实例。返回 { success: true } 或 { success: false, error }。
function applyMessageConfig(config, newConfig, generateNotifyReadme) {
  const enabled = !!newConfig.messageNotifyEnabled;

  // 逐源校验并规整为干净对象
  const rawSources = Array.isArray(newConfig.messageSources) ? newConfig.messageSources : [];
  const cleanSources = [];
  const usedIds = new Set();
  for (let i = 0; i < rawSources.length; i++) {
    const raw = rawSources[i] && typeof rawSources[i] === 'object' ? rawSources[i] : {};
    const name = String(raw.name || '').trim();
    if (!name) {
      return { success: false, error: '第 ' + (i + 1) + ' 个接口源名称不能为空' };
    }
    const sourceEnabled = !!raw.enabled;
    const listUrl = createMessageNotify.resolveListUrl(raw);
    if (sourceEnabled && !isHttpUrl(listUrl)) {
      return { success: false, error: '「' + name + '」的接口地址必须是 http 或 https URL' };
    }
    let authHeaders = '{}';
    try {
      authHeaders = normalizeMessageAuthHeaders(raw.authHeaders);
    } catch (e) {
      return { success: false, error: '「' + name + '」的请求头格式错误：' + e.message };
    }
    let id = String(raw.id || '').trim() || genSourceId();
    while (usedIds.has(id)) {
      id = genSourceId();
    }
    usedIds.add(id);
    cleanSources.push({ id, name, listUrl, authHeaders, enabled: sourceEnabled });
  }

  const filename = String(newConfig.messageNotifyVideo || '').trim() || DEFAULT_CONFIG.messageNotifyVideo;
  if (!createAssets.isSafeAssetFilename(filename)) {
    return { success: false, error: '小火箭素材只能填写文件名，不能包含路径分隔符' };
  }

  config.messageNotifyEnabled = enabled;
  config.messageSources = cleanSources;
  config.messagePollInterval = clampNumber(newConfig.messagePollInterval, 10, 3600, DEFAULT_CONFIG.messagePollInterval);
  config.messageMaxCacheItems = clampNumber(newConfig.messageMaxCacheItems, 1, 30, DEFAULT_CONFIG.messageMaxCacheItems);
  if (newConfig.messageNotifyDir !== undefined) {
    const dir = String(newConfig.messageNotifyDir || '').trim();
    if (dir) {
      try {
        const stat = fs.statSync(dir);
        if (!stat.isDirectory()) {
          return { success: false, error: '小火箭素材目录必须是有效目录' };
        }
      } catch (e) {
        return { success: false, error: '小火箭素材目录不存在或无法访问' };
      }
      config.messageNotifyDir = dir;
      if (typeof generateNotifyReadme === 'function') generateNotifyReadme(dir);
    } else {
      config.messageNotifyDir = '';
    }
  }
  config.messageNotifyVideo = filename;
  config.messageNotifyAnimation = 'rocket-corner';
  // 用户已通过多源 UI 保存，旧单源字段（messageApiBaseUrl/messageAuthHeaders）已无意义；
  // 清空以防 messageSources 被清空后，旧 baseUrl 在下次启动时把已删除的源复活。
  config.messageApiBaseUrl = '';
  config.messageAuthHeaders = '{}';
  return { success: true };
}

// 读取配置：优先用户配置，缺省回退内置默认；做迁移与 clamp；失败回退默认。返回新 config 对象。
function load({ userConfigPath, defaultConfigPath }) {
  let config;
  try {
    const configPath = fs.existsSync(userConfigPath) ? userConfigPath : defaultConfigPath;
    const data = fs.readFileSync(configPath, 'utf-8');
    config = { ...DEFAULT_CONFIG, ...JSON.parse(data) };
    migrateMessageSources(config);
    // 旧配置迁移：messageMaxCacheItems 上限由 99 收紧到 30；移除已废弃的 messageListLimit。
    config.messageMaxCacheItems = clampNumber(config.messageMaxCacheItems, 1, 30, DEFAULT_CONFIG.messageMaxCacheItems);
    delete config.messageListLimit;
  } catch (e) {
    console.error('[purr-pause] Config load failed, using defaults:', e.message);
    config = { ...DEFAULT_CONFIG };
    migrateMessageSources(config);
  }
  return config;
}

// 保存配置。isDev 为真时把 thresholdMinutes 还原为默认（防开发用的 0.15 被写入持久化）。
function save({ userConfigPath, config, isDev }) {
  try {
    const dir = path.dirname(userConfigPath);
    if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
    const toSave = { ...config };
    if (isDev) {
      toSave.thresholdMinutes = DEFAULT_CONFIG.thresholdMinutes;
    }
    fs.writeFileSync(userConfigPath, JSON.stringify(toSave, null, 2));
  } catch (e) {
    console.error('[purr-pause] Config save failed:', e.message);
  }
}

module.exports = { DEFAULT_CONFIG, load, save, applyMessageConfig };
