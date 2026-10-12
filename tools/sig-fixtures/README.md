# 序列号格式一致性校验

校验 `native/src/license/` 的序列号实现与 `tools/keygen.js`（发号端）产生**完全一致**的字节。

这是本项目最重要的一道防线：一旦签发出去的序列号无法被应用验证，
用户就会立刻失去授权，且问题往往在发码很久后才暴露。

## 序列号格式

```
payload  = 4 字节天数（BigEndian） + 8 字节机器码    （共 12 字节）
签名      = Ed25519 对 payload 的签名（64 字节）
序列号    = "PP-" + base64(payload || 签名)，每 8 字符用 "-" 分隔
```

机器码为 16 位十六进制（8 字节），由 OS、架构、CPU 型号与 `/etc/machine-id`
经 SHA-256 截取得到。

## 目录内容

| 文件 | 说明 |
|------|------|
| `fixtures.json` | 基准数据：私钥 seed、样例 payload、期望签名与序列号。**含私钥，已 gitignore** |
| `verify.rs` | Rust 侧校验程序，重签并逐字节比对 |
| `Cargo.toml` | 独立可运行的校验工程（依赖 `ed25519-dalek`） |

## 运行

```bash
cd tools/sig-fixtures
mkdir -p src && cp verify.rs src/main.rs
cargo run --quiet
# 用完清理
rm -rf src target Cargo.lock
```

## 两种运行模式

`verify.rs` 会根据 `fixtures.json` 是否存在自动选择模式：

| 模式 | 触发条件 | 覆盖范围 | 适用场景 |
|------|---------|---------|---------|
| **完整校验** | 本机有 `fixtures.json` | 公钥推导 + 签名字节逐字节比对 + 序列号字符串比对 + 解析回测 | 改动签名逻辑后 |
| **无密钥自检** | 无 `fixtures.json` | 解析顺序 + 验签链路 + 抗篡改 | 公开仓库 CI（夹具已被 gitignore） |

两种模式都以 `PASS` 结尾、退出码 0；任一断言失败则退出码 1。

### 为什么 CI 跑的是「无密钥自检」

`fixtures.json` 含Ed25519 私钥 seed，不能进公开仓库。
但完全不校验也不行——历史上最致命的bug 正是「解析顺序写反导致存量序列号全部失效」。

因此 `verify.rs` 内置了 3 组**真实签发过的序列号**作为回归向量
（序列号本身是公开数据，不含私钥信息，可安全入库），配合硬编码的**公钥**
（不含私钥）完成以下校验：

1. **解析顺序回归** —— `PP-` 前缀 → 去 `-` → base64 字符集清洗，三步顺序不得变动
2. **验签回归** —— 存量序列号（30 天 / 永久 / 365 天）仍能被验证端接受
3. **抗篡改回归** —— 改动一个字节后必须被拒绝

这三项覆盖了「用户丢失授权」的实际触发路径。签名字节的逐字节比对
只能证明发号端与验证端一致，无法用公开信息完成，故仅在本地跑。

## 覆盖范围

完整模式（需私钥）额外覆盖：

1. **公钥推导** —— 从私钥 seed 推导的公钥须等于 `license/mod.rs` 中的 `PUBLIC_KEY_SPKI`
2. **签名字节** —— 对同一 payload 重签，64 字节签名须逐字节一致
3. **序列号字符串** —— 组装出的序列号须与发号端完全相同

## 修改激活逻辑时务必重跑

以下改动都会影响已发放的序列号，改完必须执行上述校验：

- `native/src/license/mod.rs` 中的 `PUBLIC_KEY_SPKI`
- `parse_serial` / `verify_serial` 的解析与清洗顺序
- `internal_key()` 的盐值
- `tools/keygen.js` 中的密钥与序列化逻辑

## ⚠️ 两个发布后不可更改的常量

`native/src/license/mod.rs` 里有两个字符串参与 HMAC 签名计算，
**一旦有用户激活过就不能改动**，否则存量用户的 `license.json` 会校验失败：

```rust
fn internal_key() -> String {
    let digest = Sha256::digest(b"purr-pause-internal-");   // ← 不可改
    ...
}

fn secondary_mark_path() -> PathBuf {
    ...
    .join(".purr-pause-mark")                              // ← 不可改
}
```

若确实需要轮换，必须同时保留对旧值的兼容校验，否则等于让所有已激活用户失效。

## 安全提示

`fixtures.json` 含**私钥 seed**，`tools/keygen.js` 含**发号私钥**，
两者均已加入 `.gitignore`，**请勿提交到公开仓库或打进发行包**。

发号工具需要从私有备份恢复：

```bash
cp ~/purr-pause-backup-private/keygen.js tools/
```

`verify.rs` 内置的回归向量只含**已签发的序列号**与**公钥**，
两者均为公开信息，不构成密钥泄露。

> 私钥一旦公开，已发放的序列号全部可被伪造。
> 若仓库已推送到公开远端，请立即轮换密钥对
> （同时更新 `keygen.js` 与 `license/mod.rs` 的 `PUBLIC_KEY_SPKI`），
> 但注意轮换会让所有存量用户的授权失效，需提前公告。