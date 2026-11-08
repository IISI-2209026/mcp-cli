#!/usr/bin/env node
'use strict';

const crypto = require('crypto');
const fs = require('fs');
const https = require('https');
const os = require('os');
const path = require('path');

const packageJson = require('../package.json');
const repo = process.env.MCP_CLI_REPO || 'doggy8088/mcp-cli';
const version = process.env.MCP_CLI_VERSION || `v${packageJson.version}`;
const vendorDir = path.join(__dirname, '..', 'vendor');
const binaryName = process.platform === 'win32' ? 'mcp-cli.exe' : 'mcp-cli';
const binaryPath = path.join(vendorDir, binaryName);

function assetName() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === 'linux' && arch === 'x64') return 'mcp-cli-linux-x64';
  if (platform === 'linux' && arch === 'arm64') return 'mcp-cli-linux-arm64';
  if (platform === 'darwin' && arch === 'x64') return 'mcp-cli-darwin-x64';
  if (platform === 'darwin' && arch === 'arm64') return 'mcp-cli-darwin-arm64';
  // 業務動機：支援 Windows 兩大主流架構 (x64 與 ARM64)，確保絕大多數 Windows 使用者皆可順利安裝
  if (platform === 'win32' && arch === 'x64') return 'mcp-cli-win-x64.exe';
  if (platform === 'win32' && arch === 'arm64') return 'mcp-cli-win-arm64.exe';

  throw new Error(`Unsupported platform: ${platform}/${arch}`);
}

function request(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    const req = https.get(url, {
      headers: {
        'User-Agent': `${packageJson.name}/${packageJson.version}`,
      },
    }, (res) => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode) && res.headers.location) {
        res.resume();
        if (redirects >= 5) {
          reject(new Error(`Too many redirects while fetching ${url}`));
          return;
        }
        resolve(request(new URL(res.headers.location, url).toString(), redirects + 1));
        return;
      }

      if (res.statusCode !== 200) {
        res.resume();
        reject(new Error(`Request failed with status ${res.statusCode}: ${url}`));
        return;
      }

      const chunks = [];
      res.on('data', (chunk) => chunks.push(chunk));
      res.on('end', () => resolve(Buffer.concat(chunks)));
    });

    req.on('error', reject);
  });
}

async function verifyChecksum(binary, name) {
  const checksumsUrl = `https://github.com/${repo}/releases/download/${version}/checksums.txt`;

  let checksums;
  try {
    checksums = (await request(checksumsUrl)).toString('utf8');
  } catch (_) {
    return;
  }

  const line = checksums.split(/\r?\n/).find((entry) => entry.includes(name));
  if (!line) return;

  const expected = line.trim().split(/\s+/)[0];
  const actual = crypto.createHash('sha256').update(binary).digest('hex');

  if (expected !== actual) {
    throw new Error(`Checksum mismatch for ${name}`);
  }
}

async function main() {
  const name = assetName();
  const downloadUrl = `https://github.com/${repo}/releases/download/${version}/${name}`;
  const tmpPath = path.join(os.tmpdir(), `${name}-${process.pid}`);

  console.log(`Downloading ${name} from ${repo}@${version}`);
  const binary = await request(downloadUrl);
  await verifyChecksum(binary, name);

  fs.mkdirSync(vendorDir, { recursive: true });
  fs.writeFileSync(tmpPath, binary, { mode: 0o755 });
  // 業務動機：在 Windows 等多磁區環境下，若暫存目錄 (os.tmpdir) 與安裝目標目錄位於不同磁區，
  // fs.renameSync 會觸發 EXDEV 錯誤；在此透過 try-catch 降級為 copyFileSync + unlinkSync。
  try {
    fs.renameSync(tmpPath, binaryPath);
  } catch (err) {
    if (err && err.code === 'EXDEV') {
      fs.copyFileSync(tmpPath, binaryPath);
      fs.unlinkSync(tmpPath);
    } else {
      throw err;
    }
  }
  fs.chmodSync(binaryPath, 0o755);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
