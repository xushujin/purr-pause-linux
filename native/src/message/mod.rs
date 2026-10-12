//! 消息提醒：轮询第三方待办接口，差异检测新待办。
//!
//! 与 lib/message-notify.js 行为等价：
//! - 首次同步只建立基线，不弹动画
//! - 之后按 id 差集检测新增
//! - 连续失败 5 次后指数退避，最高 300s
//! - 请求头不写日志

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub level: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: String,
    #[serde(rename = "targetUrl", default)]
    pub target_url: String,
    /// 所属源 id（跨源去重用）
    #[serde(skip)]
    pub origin_id: String,
    /// 跨源唯一键：`源id::条目id`
    #[serde(skip)]
    pub uid: String,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<ApiData>,
}

#[derive(Debug, Deserialize, Default)]
struct ApiData {
    #[serde(default)]
    total: i64,
    #[serde(default)]
    items: Vec<ApiRawItem>,
}

#[derive(Debug, Deserialize)]
struct ApiRawItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    level: Option<String>,
    #[serde(rename = "createdAt", default)]
    created_at: Option<String>,
    #[serde(rename = "targetUrl", default)]
    target_url: Option<String>,
}

/// 每个接口源的运行时状态。
pub struct SourceRuntime {
    pub id: String,
    pub name: String,
    pub list_url: String,
    pub auth_headers: String,
    pub enabled: bool,

    pub snapshot: Vec<TodoItem>,
    pub badge_count: i64,
    pub total: i64,
    pub consecutive_failures: u32,
    pub last_error: String,
    pub first_sync: bool,
    pub list_in_flight: bool,
    known_ids: HashSet<String>,
    cleared_ids: HashSet<String>,
}

impl SourceRuntime {
    pub fn new(id: String, name: String, list_url: String, auth_headers: String, enabled: bool) -> Self {
        Self {
            id,
            name,
            list_url,
            auth_headers,
            enabled,
            snapshot: Vec::new(),
            badge_count: 0,
            total: 0,
            consecutive_failures: 0,
            last_error: String::new(),
            first_sync: true,
            list_in_flight: false,
            known_ids: HashSet::new(),
            cleared_ids: HashSet::new(),
        }
    }

    /// 本源 id（用于跨源归属判定）。
    pub fn origin_id(&self) -> &str {
        &self.id
    }

    /// 当前轮询间隔（秒）：失败 ≥5 次后指数退避，最高 300。
    pub fn poll_interval_secs(&self, base: i64) -> u64 {
        if self.consecutive_failures < 5 {
            return base.max(10) as u64;
        }
        let backoff = (base as f64 * 2f64.powi(self.consecutive_failures as i32 - 4)).min(300.0);
        backoff.max(10.0) as u64
    }

    fn build_url(&self, max_items: i64) -> String {
        let mut url = self.list_url.clone();
        // 追加 limit / offset，保留其余查询参数
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&format!("limit={max_items}&offset=0"));
        url
    }
}

/// 聚合状态，供 UI 渲染。
pub struct AggregatedState {
    pub enabled: bool,
    pub badge_count: i64,
    pub items: Vec<TodoItem>,
    pub sources: Vec<SourceSummary>,
    pub backoff: bool,
    pub last_error: String,
}

pub struct SourceSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub badge_count: i64,
    pub total: i64,
    pub backoff: bool,
    pub last_error: String,
}

/// 拉取单个源的待办列表。返回 (total, items)。
pub async fn fetch_source(
    client: &reqwest::Client,
    rt: &SourceRuntime,
    max_items: i64,
) -> Result<(i64, Vec<TodoItem>), String> {
    let url = rt.build_url(max_items);
    let mut req = client.get(&url).header("Accept", "application/json");

    // 自定义请求头
    let headers_json: serde_json::Value =
        serde_json::from_str(if rt.auth_headers.trim().is_empty() { "{}" } else { &rt.auth_headers })
            .map_err(|e| format!("请求头解析失败：{e}"))?;
    if let serde_json::Value::Object(map) = headers_json {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                req = req.header(k, s);
            }
        }
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;

    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let env: ApiEnvelope = serde_json::from_str(&text).map_err(|_| "响应不是合法 JSON".to_string())?;
    if env.code != 0 {
        return Err(if env.message.is_empty() {
            format!("第三方接口返回失败 (code={})", env.code)
        } else {
            env.message
        });
    }
    let data = env.data.unwrap_or_default();

    let mut items: Vec<TodoItem> = data
        .items
        .into_iter()
        .filter_map(|raw| {
            if raw.id.is_empty() || raw.title.is_empty() {
                return None;
            }
            if !is_http_url(&raw.target_url.clone().unwrap_or_default()) {
                return None;
            }
            Some(TodoItem {
                uid: format!("{}::{}", rt.id, raw.id),
                origin_id: rt.id.clone(),
                id: raw.id,
                title: raw.title,
                summary: raw.summary.unwrap_or_default(),
                level: raw.level.unwrap_or_else(|| "normal".to_string()),
                created_at: raw.created_at.unwrap_or_default(),
                target_url: raw.target_url.unwrap_or_default(),
            })
        })
        .collect();

    // 按 createdAt 倒序
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items.truncate(max_items.max(1) as usize);

    Ok((data.total, items))
}

/// 应用拉取结果，返回新增条数。
///
/// 语义：`fetched` 必须是第三方接口返回的**完整**列表（截断到 max_cache_items）。
/// 本地只做三件事：
///   1. 遗忘已被第三方删除的 clearedIds
///   2. 过滤掉本地已清理的条目
///   3. 与 knownIds 求差集得到新增
///
/// 注意：snapshot 直接反映服务端当前状态（而非与本地合并），
/// 因为第三方每次都返回完整列表；角标也等于 snapshot 长度。
pub fn apply_items(rt: &mut SourceRuntime, fetched: Vec<TodoItem>, total: i64) -> usize {
    // 1) 只保留仍存在于本次响应中的 clearedIds，集合有界
    let fetched_ids: HashSet<String> = fetched.iter().map(|i| i.id.clone()).collect();
    rt.cleared_ids.retain(|id| fetched_ids.contains(id));

    // 2) 过滤本地已清理的条目
    let snapshot: Vec<TodoItem> = fetched
        .into_iter()
        .filter(|i| !rt.cleared_ids.contains(&i.id))
        .collect();

    // 3) 差集检测新增
    let new_count = snapshot.iter().filter(|i| !rt.known_ids.contains(&i.id)).count();
    for item in &snapshot {
        rt.known_ids.insert(item.id.clone());
    }

    // known_ids 也需要收敛，否则长期运行会无限增长
    if rt.known_ids.len() > KNOWN_ID_LIMIT {
        let keep: HashSet<String> = snapshot.iter().map(|i| i.id.clone()).collect();
        rt.known_ids.retain(|id| keep.contains(id));
    }

    rt.badge_count = snapshot.len() as i64;
    rt.total = total;
    rt.snapshot = snapshot;
    new_count
}

/// 已缓存条数上限（防止 known_ids 无限增长）。
const KNOWN_ID_LIMIT: usize = 2000;
/// 本地清理记录的条数上限。
const CLEARED_ID_LIMIT: usize = 200;

/// 本地清理一条（点击后调用）。
pub fn clear_item(rt: &mut SourceRuntime, id: &str) -> bool {
    let before = rt.snapshot.len();
    rt.snapshot.retain(|i| i.id != id);
    if rt.snapshot.len() == before {
        return false;
    }
    rt.cleared_ids.insert(id.to_string());
    rt.badge_count = rt.snapshot.len() as i64;
    true
}

pub fn is_http_url(u: &str) -> bool {
    u.starts_with("http://") || u.starts_with("https://")
}

/// 聚合所有启用源的角标与列表。
pub fn aggregate(runtimes: &[SourceRuntime], global_enabled: bool) -> AggregatedState {
    let mut badge = 0i64;
    let mut items = Vec::new();
    let mut sources = Vec::new();
    let mut backoff = false;
    let mut last_error = String::new();

    for rt in runtimes {
        let eff = global_enabled && rt.enabled && !rt.list_url.is_empty();
        if eff {
            badge += rt.badge_count;
            items.extend(rt.snapshot.iter().cloned());
        }
        if rt.consecutive_failures >= 5 {
            backoff = true;
        }
        if last_error.is_empty() && !rt.last_error.is_empty() {
            last_error = format!("{}: {}", rt.name, rt.last_error);
        }
        sources.push(SourceSummary {
            id: rt.id.clone(),
            name: rt.name.clone(),
            enabled: eff,
            badge_count: rt.badge_count,
            total: rt.total,
            backoff: rt.consecutive_failures >= 5,
            last_error: rt.last_error.clone(),
        });
    }

    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    AggregatedState {
        enabled: global_enabled,
        badge_count: badge,
        items,
        sources,
        backoff,
        last_error,
    }
}

/// 测试连接（对应 testConnection）。
pub async fn test_connection(
    client: &reqwest::Client,
    list_url: &str,
    auth_headers: &str,
    max_items: i64,
) -> Result<(i64, usize), String> {
    if !is_http_url(list_url) {
        return Err("接口地址必须是 http 或 https URL".to_string());
    }
    let rt = SourceRuntime::new(
        "test".into(),
        "test".into(),
        list_url.to_string(),
        auth_headers.to_string(),
        true,
    );
    let (total, items) = fetch_source(client, &rt, max_items).await?;
    Ok((total, items.len()))
}

/// 构建默认 HTTP 客户端（5 秒超时，与 JS 版一致）。
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent("PurrPause/1.5")
        .build()
        .unwrap_or_default()
}