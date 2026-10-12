//! 序列号验证与激活管理。
//!
//! 与 `lib/license.js` 严格等价，任何改动都必须通过
//! `tools/sig-fixtures` 回归测试。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// 与 lib/license.js 中硬编码的公钥保持一致（SPKI DER）。
const PUBLIC_KEY_SPKI: &[u8] = &[
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00, 0xab, 0x1a, 0x10, 0x39,
    0xb8, 0x75, 0xb9, 0x71, 0xf5, 0x02, 0x98, 0x48, 0x6e, 0x74, 0x63, 0xe5, 0x74, 0xd0, 0x37, 0xd4,
    0xfe, 0x46, 0x81, 0xa4, 0xe7, 0x57, 0xea, 0x34, 0x35, 0x3c, 0xf0, 0x70,
];

const TRIAL_DAYS: i64 = 7;
/// usedSerials 保留上限（optimization-plan P2-8）。
const USED_SERIALS_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseStatus {
    Trial,
    Active,
    Expired,
}

#[derive(Debug, Clone)]
pub struct StatusInfo {
    pub status: LicenseStatus,
    pub kind: &'static str,
    pub days_left: i64,
    pub is_permanent: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct License {
    #[serde(rename = "installDate")]
    pub install_date: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "serialKey", default, skip_serializing_if = "Option::is_none")]
    pub serial_key: Option<String>,
    #[serde(rename = "activatedAt", default, skip_serializing_if = "Option::is_none")]
    pub activated_at: Option<String>,
    #[serde(rename = "expiresAt", default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(rename = "machineId")]
    pub machine_id: String,
    #[serde(rename = "usedSerials", default)]
    pub used_serials: Vec<String>,
    #[serde(rename = "_sig", default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

/// 内部 HMAC 密钥：由公钥与固定盐派生。
///
/// ⚠️ 这个盐参与 license.json 的完整性签名（`_sig` 字段）计算。
/// 一旦有用户激活过，**这个字符串就绝不能改动** —— 否则所有已发放的
/// license.json 会校验失败，用户丢失激活状态。
///
/// 派生出私钥后请同步更新 `tools/keygen.js` 与`tools/sig-fixtures/fixtures.json`。
fn internal_key() -> String {
    let digest = Sha256::digest(b"purr-pause-internal-");
    let full = hex::encode(digest);
    full[..32].to_string()
}

/// 生成 16 位机器码。仅 Linux：读取 /etc/machine-id。
pub fn machine_id() -> String {
    let mut parts: Vec<String> = vec![
        std::env::consts::OS.to_string(),
        std::env::consts::ARCH.to_string(),
        cpu_model(),
    ];

    let mid = fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| fs::read_to_string("/var/lib/dbus/machine-id").ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty());

    match mid {
        Some(m) => parts.push(m),
        None => {
            // 降级：主机名 + 家目录
            let host = hostname();
            let home = dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            parts.push(format!("{home}"));
            let _ = host;
        }
    }

    let joined = parts.join("|");
    let digest = Sha256::digest(joined.as_bytes());
    hex::encode(digest)[..16].to_string()
}

fn cpu_model() -> String {
    // /proc/cpuinfo 第一行 model name
    fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_default()
}

fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// 数据目录。统一复用 core::config::data_dir()，避免两处定义漂移。
fn data_dir() -> PathBuf {
    crate::core::config::data_dir()
}

fn license_path() -> PathBuf {
    data_dir().join("license.json")
}

fn install_mark_path() -> PathBuf {
    data_dir().join(".install_mark")
}

/// 试用起算时间的第二处标记文件（放在数据目录之外，防止删除数据目录重置试用）。
///
/// 该文件名参与 HMAC 签名计算，**发布后不可更改**。
fn secondary_mark_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join(".purr-pause-mark")
}

fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_iso(secs)
}

/// ISO 8601 UTC（无外部依赖，格式与 JS 的 toISOString 对齐到秒）。
fn format_iso(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86400);
    let secs_of_day = epoch_secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        y,
        m,
        d,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Howard Hinnant 的 days_from_civil 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 解析 ISO 时间串为 epoch 秒。
fn parse_iso(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: i64 = s[5..7].parse().ok()?;
    let day: i64 = s[8..10].parse().ok()?;
    let hour: i64 = s[11..13].parse().ok()?;
    let min: i64 = s[14..16].parse().ok()?;
    let sec: i64 = s[17..19].parse().ok()?;
    Some(days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 试用起算时间：双写 HMAC 标记文件，防用户删除 userData 里的标记重置试用。
fn install_date() -> String {
    let mid = machine_id();
    let primary = verify_mark(&install_mark_path(), &mid);
    if let Some(d) = primary {
        return d;
    }
    if let Some(d) = verify_mark(&secondary_mark_path(), &mid) {
        return d;
    }
    let date = iso_now();
    let sig = mark_sig(&date, &mid);
    let payload = format!("{{\"date\":\"{date}\",\"mid\":\"{mid}\",\"sig\":\"{sig}\"}}");
    let _ = write_mark(&install_mark_path(), &payload);
    let _ = write_mark(&secondary_mark_path(), &payload);
    date
}

fn mark_sig(date: &str, mid: &str) -> String {
    let key = internal_key();
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC key");
    mac.update(date.as_bytes());
    mac.update(mid.as_bytes());
    hex::encode(&mac.finalize().into_bytes()[..16])
}

fn verify_mark(path: &Path, mid: &str) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let date = v.get("date")?.as_str()?.to_string();
    let stored_mid = v.get("mid")?.as_str()?;
    let stored_sig = v.get("sig")?.as_str()?;
    if stored_mid != mid {
        return None;
    }
    if stored_sig == mark_sig(&date, mid) {
        Some(date)
    } else {
        None
    }
}

fn write_mark(path: &Path, payload: &str) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(path, payload)
}

/// license.json 的 HMAC 完整性签名。
fn sign_license(lic: &License) -> String {
    let payload = serde_json::json!({
        "installDate": lic.install_date,
        "serialKey": lic.serial_key,
        "activatedAt": lic.activated_at,
        "expiresAt": lic.expires_at,
        "type": lic.kind,
        "machineId": lic.machine_id,
        "usedSerials": lic.used_serials,
    });
    let key = internal_key();
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC key");
    mac.update(serde_json::to_string(&payload).unwrap_or_default().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify_license_integrity(lic: &License) -> bool {
    match &lic.sig {
        None => false,
        Some(s) => constant_time_eq(s.as_bytes(), sign_license(lic).as_bytes()),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn load_license() -> Option<License> {
    let path = license_path();
    let raw = fs::read_to_string(path).ok()?;
    let lic: License = serde_json::from_str(&raw).ok()?;
    if !verify_license_integrity(&lic) {
        return None;
    }
    let current = machine_id();
    if !lic.machine_id.is_empty() && lic.machine_id != current {
        return None;
    }
    Some(lic)
}

fn save_license(lic: &mut License) -> std::io::Result<()> {
    lic.sig = Some(sign_license(lic));
    let path = license_path();
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let json = serde_json::to_string_pretty(lic)?;
    fs::write(path, json)
}

/// 解析序列号。
///
/// 关键：顺序必须与 lib/license.js 的 parseSerial 完全一致 ——
/// strip `PP-` → 去所有 `-` → base64 字符集清洗 → 解码。
/// 顺序颠倒会导致前缀字符被保留、字节流错位。
pub fn parse_serial(serial: &str) -> Option<(u32, String)> {
    let s = serial.trim();
    let s = if s.len() >= 3 && s[..3].eq_ignore_ascii_case("PP-") {
        &s[3..]
    } else {
        s
    };
    let no_dash: String = s.chars().filter(|c| *c != '-').collect();
    let cleaned: String = no_dash
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    let buf = decode_base64(&cleaned)?;
    if buf.len() < 13 {
        return None;
    }
    let days = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let mid = hex::encode(&buf[4..12]);
    Some((days, mid))
}

/// 用硬编码公钥验签。
pub fn verify_serial(serial: &str) -> Option<(u32, String)> {
    let (days, mid) = parse_serial(serial)?;
    // 重新解码以取出签名部分
    let s = serial.trim();
    let s = if s.len() >= 3 && s[..3].eq_ignore_ascii_case("PP-") {
        &s[3..]
    } else {
        s
    };
    let no_dash: String = s.chars().filter(|c| *c != '-').collect();
    let cleaned: String = no_dash
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
        .collect();
    let buf = decode_base64(&cleaned)?;
    let payload = &buf[..12];
    let sig_bytes = &buf[12..];

    // SPKI DER 的最后 32 字节即为 Ed25519 公钥。
    let key_bytes: [u8; 32] = PUBLIC_KEY_SPKI[PUBLIC_KEY_SPKI.len() - 32..]
        .try_into()
        .ok()?;
    let vk = VerifyingKey::from_bytes(&key_bytes).ok()?;
    let sig = Signature::from_slice(sig_bytes).ok()?;
    vk.verify(payload, &sig).ok()?;

    Some((days, mid))
}

/// 标准 base64 编码。
#[allow(dead_code)]
fn encode_base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(T[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rev = [255u8; 256];
    for (i, &c) in table.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        let v = rev[c as usize];
        if v == 255 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        nbits += 6;
        while nbits >= 8 {
            nbits -= 8;
            out.push(((acc >> nbits) & 0xFF) as u8);
        }
    }
    Some(out)
}

pub fn check_status() -> StatusInfo {
    let now = now_secs();
    let mut lic = load_license();

    if lic.is_none() {
        lic = Some(License {
            install_date: install_date(),
            kind: "trial".to_string(),
            serial_key: None,
            activated_at: None,
            expires_at: None,
            machine_id: machine_id(),
            used_serials: Vec::new(),
            sig: None,
        });
        let mut l = lic.clone().unwrap();
        let _ = save_license(&mut l);
        lic = Some(l);
    }

    let lic = lic.unwrap();

    if lic.kind == "permanent" {
        return StatusInfo {
            status: LicenseStatus::Active,
            kind: "permanent",
            days_left: i64::MAX,
            is_permanent: true,
        };
    }

    if lic.kind != "trial" {
        if let Some(exp) = &lic.expires_at {
            let exp_secs = parse_iso(exp).unwrap_or(0);
            if now < exp_secs {
                return StatusInfo {
                    status: LicenseStatus::Active,
                    kind: if lic.kind == "monthly" {
                        "monthly"
                    } else if lic.kind == "quarterly" {
                        "quarterly"
                    } else {
                        "yearly"
                    },
                    days_left: 0,
                    is_permanent: false,
                };
            }
            return StatusInfo {
                status: LicenseStatus::Expired,
                kind: if lic.kind == "monthly" {
                    "monthly"
                } else if lic.kind == "quarterly" {
                    "quarterly"
                } else {
                    "yearly"
                },
                days_left: 0,
                is_permanent: false,
            };
        }
    }

    if lic.kind == "trial" {
        if let Some(start) = parse_iso(&lic.install_date) {
            let trial_end = start + TRIAL_DAYS * 86400;
            if now < trial_end {
                return StatusInfo {
                    status: LicenseStatus::Trial,
                    kind: "trial",
                    days_left: TRIAL_DAYS,
                    is_permanent: false,
                };
            }
            return StatusInfo {
                status: LicenseStatus::Expired,
                kind: "trial",
                days_left: 0,
                is_permanent: false,
            };
        }
    }

    StatusInfo {
        status: LicenseStatus::Expired,
        kind: "unknown",
        days_left: 0,
        is_permanent: false,
    }
}

pub struct ActivateResult {
    pub success: bool,
    pub error: Option<String>,
    pub kind: Option<String>,
    pub expires_at: Option<String>,
}

pub fn activate(serial: &str) -> ActivateResult {
    let fail = |e: &str| ActivateResult {
        success: false,
        error: Some(e.to_string()),
        kind: None,
        expires_at: None,
    };

    let parsed = match verify_serial(serial) {
        Some(p) => p,
        None => return fail("序列号无效"),
    };
    let (days, serial_mid) = parsed;

    let current_mid = machine_id();
    if serial_mid != current_mid[..16] {
        return fail("序列号与本机不匹配");
    }

    let mut lic = load_license().unwrap_or(License {
        install_date: String::new(),
        kind: "trial".to_string(),
        serial_key: None,
        activated_at: None,
        expires_at: None,
        machine_id: current_mid.clone(),
        used_serials: Vec::new(),
        sig: None,
    });

    let serial_trimmed = serial.trim().to_string();
    let serial_hash = hex::encode(Sha256::digest(serial_trimmed.as_bytes()));
    let serial_hash = serial_hash[..32].to_string();

    if lic.used_serials.contains(&serial_hash) {
        return fail("此序列号已使用过，请使用新的序列号");
    }

    lic.used_serials.push(serial_hash);
    // FIFO 淘汰，超出上限
    while lic.used_serials.len() > USED_SERIALS_LIMIT {
        lic.used_serials.remove(0);
    }
    lic.serial_key = Some(serial_trimmed);
    lic.activated_at = Some(iso_now());
    lic.machine_id = current_mid;

    if lic.install_date.is_empty() {
        lic.install_date = install_date();
    }

    if days == 0 {
        lic.kind = "permanent".to_string();
        lic.expires_at = None;
    } else {
        lic.kind = if days <= 30 {
            "monthly".to_string()
        } else if days <= 90 {
            "quarterly".to_string()
        } else {
            "yearly".to_string()
        };
        let exp = now_secs() + days as i64 * 86400;
        lic.expires_at = Some(format_iso(exp));
    }

    if save_license(&mut lic).is_err() {
        return fail("激活信息写入失败");
    }

    ActivateResult {
        success: true,
        error: None,
        kind: Some(lic.kind.clone()),
        expires_at: lic.expires_at.clone(),
    }
}

/// 供测试与调试：把 payload 与签名组装成序列号（不含签名，仅用于构造测试样例）。
#[allow(dead_code)]
pub fn format_serial_for_debug(days: u32, machine: &str) -> String {
    let mut payload = Vec::with_capacity(12);
    payload.extend_from_slice(&days.to_be_bytes());
    if let Ok(bytes) = hex::decode(machine) {
        let take = bytes.len().min(8);
        payload.extend_from_slice(&bytes[..take]);
        payload.resize(12, 0);
    }
    let b64 = encode_base64(&payload);
    let parts: Vec<&str> = b64
        .as_bytes()
        .chunks(8)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!("PP-{}", parts.join("-"))
}