use ed25519_dalek::{Signature, Signer, Verifier, SigningKey, VerifyingKey};

// PurrPause 序列号格式（与 lib/license.js 保持一致）：
//   payload  = 4 字节天数(BigEndian) + 8 字节机器码
//   serial   = "PP-" + base64(payload || signature) 按每 8 字符用 "-" 分隔
//   解析时先去掉 "PP-" 前缀与所有 "-"，base64 解码，前 12 字节为 payload，其余为签名。

fn parse_serial(serial: &str) -> Option<(u32, [u8; 8])> {
    // 关键：必须与 lib/license.js 的 parseSerial 顺序完全一致 ——
    //   1) 先剥掉 "PP-" 前缀（大小写不敏感）
    //   2) 再去掉所有 "-"
    //   3) 最后才按 base64 字符集清洗
    // 若跳过第 1 步，前缀的 'P' 'P' 会被当成合法 base64 字符保留，导致整条字节流错位。
    let s = serial.trim();
    let s = if s.len() >= 3 && s[..3].eq_ignore_ascii_case("PP-") {
        &s[3..]
    } else {
        s
    };
    let cleaned: String = s.chars().filter(|c| *c != '-').collect();
    let cleaned: String = cleaned
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
    let mut mid = [0u8; 8];
    mid.copy_from_slice(&buf[4..12]);
    Some((days, mid))
}

// 标准 base64 解码。6 位分组跨字节累积后按 8 位切出。
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rev = [255u8; 256];
    for (i, &c) in T.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    for &c in s.as_bytes() {
        if c == b'=' {
            break; // 填充，剩余位丢弃
        }
        let v = rev[c as usize];
        if v == 255 {
            return None;
        }
        // 左移累积：每取入 6 位就可能吐出一个整字节
        acc = (acc << 6) | v as u32;
        nbits += 6;
        while nbits >= 8 {
            nbits -= 8;
            out.push(((acc >> nbits) & 0xFF) as u8);
        }
    }
    Some(out)
}

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

fn format_serial(raw: &[u8]) -> String {
    let b64 = encode_base64(raw);
    let parts: Vec<&str> = b64
        .as_bytes()
        .chunks(8)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!("PP-{}", parts.join("-"))
}

fn main() {
    let raw = match std::fs::read_to_string("fixtures.json") {
        Ok(r) => r,
        Err(_) => {
            // fixtures.json 含私钥 seed，已被 .gitignore 排除，公开仓库的 CI 拿不到。
            // 这时退化为「无密钥自检」：只验证序列号格式解析逻辑本身，
            // 签名字节比对留给持有夹具的本地/私有流水线。
            println!(
                "未找到 fixtures.json（该文件含私钥 seed，已 gitignore）。\n\
                 跳过签名字节比对，仅执行序列号格式自检。\n\
                 如需完整校验，请在本机放置 fixtures.json 后重跑。"
            );
            selftest_without_fixture();
            return;
        }
    };

    // 极简取值：从夹具中提取 seed 与 cases（避免引入 serde 依赖）
    let seed_hex = raw
        .split("\"seed\": \"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("seed 缺失")
        .trim()
        .to_string();
    let seed_bytes = hex::decode(&seed_hex).expect("seed hex 非法");
    let mut seed_arr = [0u8; 32];
    seed_arr.copy_from_slice(&seed_bytes);

    // 夹具中的 payload 列表（注意：JSON 结构里 "payload" 只出现在 cases 内，
    // 但 "seed" 的值里不含这些 key，因此直接匹配即可，不应 skip）
    fn field_all(raw: &str, key: &str) -> Vec<String> {
        let pat = format!("\"{}\": \"", key);
        raw.lines()
            .filter_map(|line| line.split(&pat).nth(1))
            .filter_map(|rest| rest.split('"').next())
            .map(|s| s.trim().to_string())
            .collect()
    }
    let payloads = field_all(&raw, "payload");
    let sigs = field_all(&raw, "signature");
    let serials = field_all(&raw, "serial");
    assert_eq!(payloads.len(), sigs.len(), "payload/signature 数量不一致");
    assert_eq!(payloads.len(), serials.len(), "payload/serial 数量不一致");
    println!("\n从夹具解析到 {} 组测试用例", payloads.len());

    println!("=== 1. 从私钥 seed 推导公钥 ===");
    let sk = SigningKey::from_bytes(&seed_arr);
    let vk: VerifyingKey = sk.verifying_key();
    let derived = vk.to_bytes();
    println!("Rust 推导公钥 (hex): {}", hex::encode(derived));
    println!("Node  推导公钥 (hex): ab1a1039b875b971f50298486e7463e574d037d4fe4681a4e757ea34353cf070");

    println!("\n=== 2. 重签并逐字节比对 ===");
    let mut all_pass = true;
    for i in 0..payloads.len() {
        let payload = hex::decode(payloads[i].as_str()).unwrap();
        let expected_sig = hex::decode(sigs[i].as_str()).unwrap();
        let expected_serial = serials[i].as_str();

        let sig: Signature = sk.sign(&payload);
        let sig_bytes = sig.to_bytes();
        let sig_match = sig_bytes.to_vec() == expected_sig;

        // 重新组装序列号并比对字符串
        let mut combined = payload.clone();
        combined.extend_from_slice(&sig_bytes);
        let rebuilt = format_serial(&combined);
        let serial_match = rebuilt == expected_serial;

        // 顺带验证自验签
        let verify_ok = vk.verify(&payload, &sig).is_ok();

        println!(
            "case {}: 签名字节一致={} 序列号字符串一致={} 自验签={}",
            i,
            if sig_match { "PASS" } else { "FAIL" },
            if serial_match { "PASS" } else { "FAIL" },
            if verify_ok { "PASS" } else { "FAIL" }
        );
        if !sig_match || !serial_match || !verify_ok {
            all_pass = false;
            println!("  期望 sig: {}", sigs[i]);
            println!("  实际 sig: {}", hex::encode(sig_bytes));
            println!("  期望 serial: {}", expected_serial);
            println!("  实际 serial: {}", rebuilt);
        }
    }

    println!("\n=== 3. 解析路径回测（Rust 解析 Node 生成的序列号）===");
    for s in &serials {
        let cleaned = s.as_str();
        match parse_serial(cleaned) {
            Some((days, mid)) => {
                println!(
                    "PASS days={} machine_id={}",
                    days,
                    hex::encode(mid)
                );
            }
            None => {
                println!("FAIL 无法解析: {}", cleaned);
                all_pass = false;
            }
        }
    }

    println!("\n=== 总判定: {} ===", if all_pass { "PASS 跨语言签名完全等价" } else { "FAIL 存在差异" });
    if !all_pass {
        std::process::exit(1);
    }
}

/// 无 fixtures.json 时的降级自检（公开 CI 使用）。
///
/// 这里刻意**不含任何私钥**：只用公开公钥验证「已发放序列号能否被验证端接受」，
/// 覆盖真正会导致用户丢失授权的两个环节：
///   1. parse_serial 的清洗顺序（PP- 前缀 / '-' / base64 字符集）
///   2. 公钥常量是否与发号端一致、签名能否验通
/// 签名字节的逐字节比对需要私钥 seed，只能在持有 fixtures.json 的环境执行。
fn selftest_without_fixture() {
    // 与 native/src/license/mod.rs 的 PUBLIC_KEY_SPKI 末 32 字节一致
    const PUBLIC_KEY: [u8; 32] = [
        0xab, 0x1a, 0x10, 0x39, 0xb8, 0x75, 0xb9, 0x71, 0xf5, 0x02, 0x98, 0x48, 0x6e, 0x74, 0x63,
        0xe5, 0x74, 0xd0, 0x37, 0xd4, 0xfe, 0x46, 0x81, 0xa4, 0xe7, 0x57, 0xea, 0x34, 0x35, 0x3c,
        0xf0, 0x70,
    ];

    // 真实签发过的序列号（由 tools/keygen.js 生成）。序列号本身是公开数据，
    // 不含私钥信息，可安全入库作为回归向量。
    const CASES: &[(&str, u32, &str)] = &[
        (
            "PP-AAAAHqGy-w9Tl9niQ-aPpQaTTx-aXJtnqie-VuK8Yq8h-r3MQUulM-m8k/Sqdd-AZPNtc/H-TUZNuZUD-ibPRYAuz-73UaQSms-4M2tNB9K-kbmcCA==",
            30,
            "a1b2c3d4e5f67890",
        ),
        (
            "PP-AAAAAAAR-IjNEVWZ3-QTy9ib7k-3Z9FNltq-QEoWPoZ9-kN2KojNR-Blowj0U1-uQd3rXRP-4TQaLPEg-6azjpCMT-hL1ZumE0-iqzMrgMs-gCKbCA==",
            0,
            "0011223344556677",
        ),
        (
            "PP-AAABbf/u-3cy7qpmI-IeS4zW2g-7NZiGLJf-zI4f4CKu-pl4Briwr-xDeE/OIc-O/GTU7c1-W/ptKCXT-QsGrBZk1-LthjV1az-UJfdReR9-dsMbAA==",
            365,
            "ffeeddccbbaa9988",
        ),
    ];

    let vk = match VerifyingKey::from_bytes(&PUBLIC_KEY) {
        Ok(k) => k,
        Err(e) => {
            println!("FAIL 公钥非法：{e}");
            std::process::exit(1);
        }
    };
    println!("\n公钥 (hex): {}", hex::encode(PUBLIC_KEY));

    println!("\n=== 1. 解析顺序回归（PP- 前缀 / '-' / base64 清洗）===");
    let mut all_pass = true;
    for (serial, want_days, want_mid) in CASES {
        match parse_serial(serial) {
            Some((days, mid)) => {
                let got_mid = hex::encode(mid);
                let ok = days == *want_days && got_mid == *want_mid;
                println!(
                    "  {} days={} machine_id={}",
                    if ok { "PASS" } else { "FAIL" },
                    days,
                    got_mid
                );
                if !ok {
                    println!("    期望 days={} machine_id={}", want_days, want_mid);
                    all_pass = false;
                }
            }
            None => {
                println!("  FAIL 无法解析: {serial}");
                all_pass = false;
            }
        }
    }

    println!("\n=== 2. 验签回归（存量序列号是否仍可被验证端接受）===");
    for (serial, want_days, _) in CASES {
        // 复刻 parse_serial 的清洗过程以取出签名字节
        let s = serial.trim();
        let s = if s.len() >= 3 && s[..3].eq_ignore_ascii_case("PP-") {
            &s[3..]
        } else {
            s
        };
        let cleaned: String = s.chars().filter(|c| *c != '-').collect();
        let cleaned: String = cleaned
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
            .collect();
        let Some(buf) = decode_base64(&cleaned) else {
            println!("  FAIL base64 解码失败: {serial}");
            all_pass = false;
            continue;
        };
        if buf.len() != 12 + 64 {
            println!("  FAIL 字节长度异常 {}（期望 76）: {serial}", buf.len());
            all_pass = false;
            continue;
        }
        let (payload, sig_bytes) = buf.split_at(12);
        let sig = Signature::from_slice(sig_bytes).expect("签名长度应为 64");
        match vk.verify(payload, &sig) {
            Ok(()) => println!("  PASS 验签通过 (days={want_days})"),
            Err(e) => {
                println!("  FAIL 验签失败: {e}");
                all_pass = false;
            }
        }
    }

    println!("\n=== 3. 抗篡改回归（改动一个字节应被拒绝）===");
    if let Some((serial, _, _)) = CASES.first() {
        // 只篡改 payload 区间内的 base64 数据字符，避开分隔符 '-' 与尾部填充 '='。
        // 索引 7 落在 "PP-AAAAH..." 的第 5 个数据字符上（索引 11 是分隔符，不可动）。
        let mut bytes = serial.as_bytes().to_vec();
        let target = 7usize;
        debug_assert!(bytes[target] != b'-');
        bytes[target] = if bytes[target] == b'A' { b'B' } else { b'A' };
        // 改单个 ASCII 字节，长度必然不变
        let tampered = String::from_utf8(bytes).expect("序列号为 ASCII");
        assert_eq!(tampered.len(), serial.len());

        let s = tampered.trim();
        let s = if s.len() >= 3 && s[..3].eq_ignore_ascii_case("PP-") {
            &s[3..]
        } else {
            s
        };
        let cleaned: String = s.chars().filter(|c| *c != '-').collect();
        let cleaned: String = cleaned
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/' || *c == '=')
            .collect();
        match decode_base64(&cleaned) {
            Some(buf) if buf.len() == 76 => {
                let (payload, sig_bytes) = buf.split_at(12);
                let sig = Signature::from_slice(sig_bytes).expect("签名长度应为 64");
                if vk.verify(payload, &sig).is_err() {
                    println!("  PASS 篡改后已被拒绝");
                } else {
                    println!("  FAIL 篡改后竟验签通过 —— 危险");
                    all_pass = false;
                }
            }
            Some(buf) => {
                println!("  FAIL 篡改后字节数 {}（期望 76）", buf.len());
                all_pass = false;
            }
            None => println!("  FAIL 篡改样本无法 base64 解码"),
        }
    }

    println!(
        "\n=== 总判定: {} ===",
        if all_pass {
            "PASS 序列号格式与验签链路正常（未做签名字节比对）"
        } else {
            "FAIL 存在差异"
        }
    );
    if !all_pass {
        std::process::exit(1);
    }
}