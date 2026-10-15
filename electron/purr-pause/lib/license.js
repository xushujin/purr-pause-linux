const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const os = require('os');

const PUBLIC_KEY = `-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAqxoQObh1uXH1AphIbnRj5XTQN9T+RoGk51fqNDU88HA=
-----END PUBLIC KEY-----`;

const INTERNAL_KEY = crypto.createHash('sha256')
  .update('purr-pause-internal-' + PUBLIC_KEY).digest('hex').substring(0, 32);
const TRIAL_DAYS = 7;

function getMachineId() {
  const parts = [
    os.platform(),
    os.arch(),
    (os.cpus()[0] || {}).model || ''
  ];
  try {
    if (os.platform() === 'linux') {
      const mid = fs.readFileSync('/etc/machine-id', 'utf-8').trim();
      parts.push(mid);
    } else if (os.platform() === 'darwin') {
      const { execFileSync } = require('child_process');
      const out = execFileSync('ioreg', ['-rd1', '-c', 'IOPlatformExpertDevice'], { timeout: 3000 });
      const match = out.toString().match(/"IOPlatformUUID"\s*=\s*"([^"]+)"/);
      if (match) parts.push(match[1]);
    } else if (os.platform() === 'win32') {
      const { execFileSync } = require('child_process');
      const out = execFileSync('reg', ['query', 'HKLM\\SOFTWARE\\Microsoft\\Cryptography', '/v', 'MachineGuid'], { timeout: 3000 });
      const match = out.toString().match(/MachineGuid\s+REG_SZ\s+(.+)/);
      if (match) parts.push(match[1].trim());
    }
  } catch (e) {
    parts.push(os.hostname() + os.homedir());
  }
  return crypto.createHash('sha256').update(parts.join('|')).digest('hex').substring(0, 16);
}

function getLicensePath(userDataPath) {
  return path.join(userDataPath, 'license.json');
}

function getInstallMarkPath(userDataPath) {
  return path.join(userDataPath, '.install_mark');
}


function getSecondaryMarkPath() {
  const base = os.platform() === 'win32'
    ? path.join(os.homedir(), 'AppData', 'Local')
    : path.join(os.homedir(), '.local', 'share');
  return path.join(base, '.purr-pause-mark');
}

function getInstallDate(userDataPath) {
  const markPath = getInstallMarkPath(userDataPath);
  const secondaryPath = getSecondaryMarkPath();
  const machineId = getMachineId();

  function verifyMark(filePath) {
    try {
      if (!fs.existsSync(filePath)) return null;
      const raw = fs.readFileSync(filePath, 'utf-8');
      const data = JSON.parse(raw);
      const expectedSig = crypto.createHmac('sha256', INTERNAL_KEY)
        .update(data.date + data.mid).digest('hex').substring(0, 16);
      if (data.sig === expectedSig && data.mid === machineId) {
        return data.date;
      }
    } catch (e) {}
    return null;
  }

  const primary = verifyMark(markPath);
  if (primary) return primary;
  const secondary = verifyMark(secondaryPath);
  if (secondary) return secondary;

  const date = new Date().toISOString();
  const sig = crypto.createHmac('sha256', INTERNAL_KEY)
    .update(date + machineId).digest('hex').substring(0, 16);
  const markData = JSON.stringify({ date, mid: machineId, sig });

  function writeMark(filePath) {
    try {
      const dir = path.dirname(filePath);
      if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(filePath, markData);
    } catch (e) {}
  }

  writeMark(markPath);
  writeMark(secondaryPath);
  return date;
}


function verifySerial(serial) {
  try {
    const cleaned = serial.replace(/[^A-Za-z0-9+/=]/g, '');
    if (!cleaned) return null;
    const buf = Buffer.from(cleaned, 'base64');
    if (buf.length < 13) return null;
    const payload = buf.subarray(0, 12);
    const signature = buf.subarray(12);
    const pubKey = crypto.createPublicKey(PUBLIC_KEY);
    const valid = crypto.verify(null, payload, pubKey, signature);
    if (!valid) return null;
    const days = payload.readUInt32BE(0);
    const machineId = payload.subarray(4, 12).toString('hex');
    return { days, machineId };
  } catch (e) {
    return null;
  }
}

function parseSerial(serial) {
  const stripped = serial.trim().replace(/^PP-/i, '').replace(/-/g, '');
  return verifySerial(stripped);
}


function signLicense(data) {
  const payload = JSON.stringify({
    installDate: data.installDate,
    serialKey: data.serialKey,
    activatedAt: data.activatedAt,
    expiresAt: data.expiresAt,
    type: data.type,
    machineId: data.machineId,
    usedSerials: data.usedSerials || []
  });
  return crypto.createHmac('sha256', INTERNAL_KEY).update(payload).digest('hex');
}

function verifyLicenseIntegrity(data) {
  if (!data || !data._sig) return false;
  const expected = signLicense(data);
  try {
    return crypto.timingSafeEqual(Buffer.from(data._sig, 'hex'), Buffer.from(expected, 'hex'));
  } catch (e) {
    return false;
  }
}

function loadLicense(userDataPath) {
  const licensePath = getLicensePath(userDataPath);
  try {
    if (fs.existsSync(licensePath)) {
      const data = JSON.parse(fs.readFileSync(licensePath, 'utf-8'));
      if (!verifyLicenseIntegrity(data)) return null;
      if (data.machineId && data.machineId !== getMachineId()) return null;
      return data;
    }
  } catch (e) {}
  return null;
}

function saveLicense(userDataPath, data) {
  const licensePath = getLicensePath(userDataPath);
  const dir = path.dirname(licensePath);
  if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
  data._sig = signLicense(data);
  fs.writeFileSync(licensePath, JSON.stringify(data, null, 2));
}


function checkStatus(userDataPath) {
  let lic = loadLicense(userDataPath);
  const now = new Date();

  if (!lic) {
    const installDate = getInstallDate(userDataPath);
    lic = {
      installDate,
      type: 'trial',
      serialKey: null,
      activatedAt: null,
      expiresAt: null,
      machineId: getMachineId()
    };
    saveLicense(userDataPath, lic);
  }

  if (lic.type === 'permanent') {
    return { status: 'active', type: 'permanent', daysLeft: Infinity };
  }

  if (lic.type !== 'trial' && lic.expiresAt) {
    const expires = new Date(lic.expiresAt);
    if (now < expires) {
      const todayStart = new Date(now);
      todayStart.setHours(0, 0, 0, 0);
      const expireDay = new Date(expires);
      expireDay.setHours(0, 0, 0, 0);
      const daysLeft = Math.round((expireDay - todayStart) / (1000 * 60 * 60 * 24));
      return { status: 'active', type: lic.type, daysLeft };
    } else {
      return { status: 'expired', type: lic.type, daysLeft: 0 };
    }
  }

  if (lic.type === 'trial') {
    const installDate = new Date(lic.installDate);
    const trialEnd = new Date(installDate);
    trialEnd.setDate(trialEnd.getDate() + TRIAL_DAYS);
    trialEnd.setHours(23, 59, 59, 999);
    if (now < trialEnd) {
      const todayStart = new Date(now);
      todayStart.setHours(0, 0, 0, 0);
      const endDay = new Date(trialEnd);
      endDay.setHours(0, 0, 0, 0);
      const daysLeft = Math.round((endDay - todayStart) / (1000 * 60 * 60 * 24));
      return { status: 'trial', type: 'trial', daysLeft };
    } else {
      return { status: 'expired', type: 'trial', daysLeft: 0 };
    }
  }

  return { status: 'expired', type: 'unknown', daysLeft: 0 };
}

function hashSerial(serial) {
  return crypto.createHash('sha256').update(serial.trim()).digest('hex').substring(0, 32);
}

function activate(userDataPath, serial) {
  const result = parseSerial(serial);
  if (!result) return { success: false, error: '序列号无效' };

  const currentMid = getMachineId();
  if (result.machineId !== currentMid.substring(0, 16)) {
    return { success: false, error: '序列号与本机不匹配' };
  }

  const now = new Date();
  const lic = loadLicense(userDataPath) || {};

  const serialHash = hashSerial(serial);
  const usedSerials = lic.usedSerials || [];
  if (usedSerials.includes(serialHash)) {
    return { success: false, error: '此序列号已使用过，请使用新的序列号' };
  }

  usedSerials.push(serialHash);
  lic.usedSerials = usedSerials;
  lic.serialKey = serial;
  lic.activatedAt = now.toISOString();
  lic.machineId = currentMid;

  if (!lic.installDate) {
    lic.installDate = getInstallDate(userDataPath);
  }

  if (result.days === 0) {
    lic.type = 'permanent';
    lic.expiresAt = null;
  } else {
    if (result.days <= 30) lic.type = 'monthly';
    else if (result.days <= 90) lic.type = 'quarterly';
    else lic.type = 'yearly';
    lic.expiresAt = new Date(now.getTime() + result.days * 24 * 60 * 60 * 1000).toISOString();
  }

  saveLicense(userDataPath, lic);
  return { success: true, type: lic.type, expiresAt: lic.expiresAt };
}

module.exports = {
  verifySerial: parseSerial,
  checkStatus,
  activate,
  getMachineId,
  TRIAL_DAYS
};




