#!/usr/bin/env node
const crypto = require('crypto');

const PRIVATE_KEY = `-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIFAU1AWSlPSuOMhZxc7wgpQT4hc6nyhYc8k7ohGZ4q5z
-----END PRIVATE KEY-----`;

const args = process.argv.slice(2);

function printUsage() {
  console.log(`
胖猫暂停一下（PurrPause） 序列号生成器（开发者工具）

用法:
  node tools/keygen.js --mid <机器码> --days <天数>       生成限时序列号
  node tools/keygen.js --mid <机器码> --permanent         生成永久序列号
  node tools/keygen.js --verify <序列号>                  验证序列号

参数:
  --mid <机器码>     用户提供的 16 位机器码（必填）
  --days <天数>      授权天数
  --permanent        永久授权
  --verify <序列号>  验证序列号有效性

示例:
  node tools/keygen.js --mid a1b2c3d4e5f67890 --days 30
  node tools/keygen.js --mid a1b2c3d4e5f67890 --permanent
`);
}

function getArg(name) {
  const idx = args.indexOf(name);
  if (idx === -1) return null;
  return args[idx + 1] || null;
}

function hasFlag(name) {
  return args.includes(name);
}

function generateSerial(days, machineId) {
  const midBuf = Buffer.from(machineId, 'hex');
  if (midBuf.length < 8) throw new Error('机器码必须至少 16 位十六进制');
  const payload = Buffer.alloc(12);
  payload.writeUInt32BE(days, 0);
  midBuf.copy(payload, 4, 0, 8);
  const privKey = crypto.createPrivateKey(PRIVATE_KEY);
  const signature = crypto.sign(null, payload, privKey);
  const combined = Buffer.concat([payload, signature]);
  return formatSerial(combined);
}

function formatSerial(raw) {
  const b64 = raw.toString('base64');
  const parts = [];
  for (let i = 0; i < b64.length; i += 8) {
    parts.push(b64.substring(i, i + 8));
  }
  return 'PP-' + parts.join('-');
}

function verifySerial(serial) {
  const PUBLIC_KEY = `-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAqxoQObh1uXH1AphIbnRj5XTQN9T+RoGk51fqNDU88HA=
-----END PUBLIC KEY-----`;
  try {
    const stripped = serial.replace(/^PP-/i, '').replace(/-/g, '');
    const buf = Buffer.from(stripped, 'base64');
    if (buf.length < 13) return null;
    const payload = buf.subarray(0, 12);
    const signature = buf.subarray(12);
    const pubKey = crypto.createPublicKey(PUBLIC_KEY);
    const valid = crypto.verify(null, payload, pubKey, signature);
    if (!valid) return null;
    return {
      days: payload.readUInt32BE(0),
      machineId: payload.subarray(4, 12).toString('hex')
    };
  } catch (e) {
    return null;
  }
}

if (args.length === 0 || hasFlag('--help') || hasFlag('-h')) {
  printUsage();
  process.exit(0);
}

if (hasFlag('--verify')) {
  const serial = getArg('--verify');
  if (!serial) {
    console.error('请提供序列号');
    process.exit(1);
  }
  const result = verifySerial(serial);
  if (result) {
    console.log('✓ 序列号有效');
    console.log(`  类型: ${result.days === 0 ? '永久' : result.days + ' 天'}`);
    console.log(`  绑定机器码: ${result.machineId}`);
  } else {
    console.log('✗ 序列号无效');
  }
  process.exit(0);
}

const machineId = getArg('--mid');
if (!machineId || machineId.length < 16) {
  console.error('错误: 必须提供 --mid <16位机器码>');
  console.error('机器码可在用户的激活窗口中查看并复制');
  process.exit(1);
}

let days = 0;
if (hasFlag('--permanent')) {
  days = 0;
} else if (hasFlag('--days')) {
  days = parseInt(getArg('--days'), 10);
  if (!days || days < 1) {
    console.error('天数必须大于 0');
    process.exit(1);
  }
} else {
  printUsage();
  process.exit(1);
}

console.log(`\n机器码: ${machineId}`);
console.log(`类型: ${days === 0 ? '永久' : days + ' 天'}`);
const serial = generateSerial(days, machineId);
console.log(`\n序列号:\n  ${serial}\n`);
