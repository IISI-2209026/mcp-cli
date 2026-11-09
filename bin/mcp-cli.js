#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const binaryName = process.platform === 'win32' ? 'mcp-cli.exe' : 'mcp-cli';
const binaryPath = path.join(__dirname, '..', 'vendor', binaryName);

if (!fs.existsSync(binaryPath)) {
  console.error('mcp-cli native binary is not installed.');
  console.error('Run `npm rebuild @patrick85081/mcp-cli` to download it again.');
  process.exit(1);
}

const child = spawn(binaryPath, process.argv.slice(2), { stdio: 'inherit' });

child.on('error', (error) => {
  console.error(error.message);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
