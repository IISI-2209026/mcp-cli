# npm 發布流程與維護知識庫

本文件說明 `mcp-cli` 如何透過 npm 發布為 `@patrick85081/mcp-cli`，並整理發布、維護、除錯、版本管理與 GitHub Actions trusted publishing 所需的技術知識。

`@patrick85081/mcp-cli` 是原生 Rust CLI 的 npm wrapper。npm package 本身不包含預先打包的所有平台二進位檔，也不會在使用者機器上執行 `cargo build`。安裝時會根據使用者平台，從 GitHub Release 下載同版本的原生 `mcp-cli` binary。

* * *

## 核心結論

**npm package version 必須與 GitHub Release tag version 對齊。**

**npm trusted publishing 不需要 `NPM_TOKEN`，但 npm registry 端必須先設定 trusted publisher。**

**npm 已發布的版本不可覆寫；GitHub Release 可以重建，但 npm package version 不可重新發布。**

**如果 release asset 缺失，npm install 會在 `postinstall` 階段失敗。**

* * *

## 專案發布模型

本專案有兩個發布層：

| 層級 | 用途 | 產物 |
|---|---|---|
| GitHub Release | 發布原生 Rust binaries | `mcp-cli-linux-x64` 等 assets |
| npm registry | 發布 Node wrapper | `@patrick85081/mcp-cli` |

npm package 是 wrapper，不是 Rust crate。

安裝流程如下：

```text
npm install -g @patrick85081/mcp-cli
        |
        v
執行 package.json scripts.postinstall
        |
        v
node scripts/install.js
        |
        v
偵測 process.platform 與 process.arch
        |
        v
組合 GitHub Release asset URL
        |
        v
下載 binary 到 vendor/mcp-cli 或 vendor/mcp-cli.exe
        |
        v
bin/mcp-cli.js 執行 vendor binary
```

* * *

## 相關檔案

| 檔案 | 說明 |
|---|---|
| `package.json` | npm package metadata、bin mapping、postinstall script、版本號 |
| `bin/mcp-cli.js` | 使用者執行 `mcp-cli` 時的 Node wrapper entrypoint |
| `scripts/install.js` | npm 安裝時下載對應平台 binary 的 script |
| `.github/workflows/release.yml` | 建置原生 binaries 並建立 GitHub Release |
| `.github/workflows/npm-publish.yml` | 使用 trusted publishing 發布 npm package |
| `scripts/release.sh` | 本機 release script，更新版本、建立 tag、push |
| `Cargo.toml` | Rust crate metadata 與 Rust CLI 版本 |
| `Cargo.lock` | Rust dependency lockfile |
| `README.md` | 使用者安裝與使用說明 |

* * *

## npm package metadata

`package.json` 的關鍵設定如下：

```json
{
  "name": "@patrick85081/mcp-cli",
  "version": "0.1.0",
  "bin": {
    "mcp-cli": "bin/mcp-cli.js"
  },
  "scripts": {
    "postinstall": "node scripts/install.js"
  },
  "engines": {
    "node": ">=18"
  },
  "files": [
    "bin/",
    "scripts/",
    "README.md",
    "LICENSE"
  ]
}
```

### `name`

npm package name 是：

```text
@patrick85081/mcp-cli
```

這是 scoped package。首次發布 scoped public package 時必須使用：

```sh
npm publish --access public
```

否則 npm 可能會把 scoped package 視為 private package，而 private package 需要付費帳號或組織設定。

### `version`

npm package version 必須與 GitHub Release tag 對齊。

```text
package.json version 1.0.0 -> GitHub tag v1.0.0
```

安裝器會用 package version 推導 release tag：

```js
const version = process.env.MCP_CLI_VERSION || `v${packageJson.version}`;
```

所以 `@patrick85081/mcp-cli@1.0.0` 預設會下載 `v1.0.0` 的 assets。

### `bin`

`bin` 設定讓 npm 在 global install 或 `npx` 情境建立 executable shim。

```json
"bin": {
  "mcp-cli": "bin/mcp-cli.js"
}
```

使用者安裝後可以執行：

```sh
mcp-cli --version
```

### `postinstall`

`postinstall` 在 npm 安裝 package 後執行。

```json
"postinstall": "node scripts/install.js"
```

此 script 負責下載原生 binary。這表示如果使用者使用以下方式跳過 lifecycle scripts，binary 不會被下載：

```sh
npm install -g @patrick85081/mcp-cli --ignore-scripts
```

這種情況下執行 `mcp-cli` 會失敗，並提示重新執行：

```sh
npm rebuild @patrick85081/mcp-cli
```

### `files`

`files` 控制 npm package 實際包含哪些檔案。

目前應包含：

```text
bin/
scripts/
README.md
LICENSE
```

不應包含：

```text
target/
node_modules/
vendor/
dist/
.git/
```

`vendor/` 不應被發布，因為 binary 應在安裝時下載，而不是把所有平台 binary 放進 npm package。

* * *

## Node wrapper 設計

`bin/mcp-cli.js` 的責任很窄：

1. 找出已下載的原生 binary。
2. 確認 binary 存在。
3. 使用 `child_process.spawn` 轉交所有 CLI arguments。
4. 使用 `stdio: 'inherit'` 保留原 CLI 的 stdout、stderr、stdin 行為。
5. 將原生 process exit code 或 signal 傳回呼叫端。

這種設計的優點：

| 優點 | 說明 |
|---|---|
| 行為接近原生 CLI | stdout、stderr、stdin 不會被 wrapper 改寫 |
| 支援 shell pipeline | `mcp-cli ... | jq` 這類用法可正常運作 |
| 避免 Node 重新實作 CLI 邏輯 | Node 只負責 dispatch |
| 易於維護 | Rust CLI 與 npm wrapper 邏輯分離 |

wrapper 不應解析 `mcp-cli` 的業務參數。所有 CLI semantics 都應留在 Rust binary。

* * *

## 安裝器設計

`scripts/install.js` 的責任：

1. 讀取 `package.json` version。
2. 偵測平台。
3. 將平台映射到 GitHub Release asset name。
4. 下載 binary。
5. 下載並驗證 `checksums.txt`，如果可取得。
6. 將 binary 寫入 `vendor/`。
7. 設定 executable permission。

### 平台偵測來源

Node 提供：

```js
process.platform
process.arch
```

常見值：

| 作業系統 | `process.platform` |
|---|---|
| Linux | `linux` |
| macOS | `darwin` |
| Windows | `win32` |

常見 CPU 架構：

| 架構 | `process.arch` |
|---|---|
| x64 | `x64` |
| ARM64 | `arm64` |

### 目前支援的平台

| 平台 | Node platform/arch | Release asset |
|---|---|---|
| Linux x64 | `linux/x64` | `mcp-cli-linux-x64` |
| Linux ARM64 | `linux/arm64` | `mcp-cli-linux-arm64` |
| macOS Intel | `darwin/x64` | `mcp-cli-darwin-x64` |
| macOS Apple Silicon | `darwin/arm64` | `mcp-cli-darwin-arm64` |
| Windows ARM64 | `win32/arm64` | `mcp-cli-win-arm64.exe` |

不支援的平台會拋出錯誤：

```text
Unsupported platform: <platform>/<arch>
```

### Windows 檔名

Windows binary 使用 `.exe`：

```text
mcp-cli.exe
```

wrapper 會根據平台決定本機 binary path：

```js
const binaryName = process.platform === 'win32' ? 'mcp-cli.exe' : 'mcp-cli';
```

### 下載來源

預設 repository：

```text
iisi-2209026/mcp-cli
```

預設 tag：

```text
v<package.json version>
```

組合後的 URL 格式：

```text
https://github.com/iisi-2209026/mcp-cli/releases/download/v<VERSION>/<ASSET>
```

### 環境變數覆寫

安裝器支援覆寫 repository 與版本：

| 環境變數 | 用途 |
|---|---|
| `MCP_CLI_REPO` | 指定 GitHub repository，例如 `owner/repo` |
| `MCP_CLI_VERSION` | 指定 release tag，例如 `v1.0.0` |

範例：

```sh
MCP_CLI_REPO=iisi-2209026/mcp-cli MCP_CLI_VERSION=v1.0.0 npm install -g @patrick85081/mcp-cli
```

這對測試 fork 或測試特定 release 有用。

* * *

## Checksum 驗證

release workflow 會產生：

```text
checksums.txt
```

格式通常類似：

```text
<sha256>  mcp-cli-linux-x64
<sha256>  mcp-cli-linux-arm64
<sha256>  mcp-cli-darwin-x64
<sha256>  mcp-cli-darwin-arm64
<sha256>  mcp-cli-win-arm64.exe
```

`scripts/install.js` 會嘗試下載 `checksums.txt`。

如果可以下載且找到對應 asset，會驗證 SHA-256。

如果 `checksums.txt` 不存在或找不到對應 asset，目前行為是跳過 checksum 驗證。

這是為了保留安裝彈性，但發布流程應視 `checksums.txt` 為必要 release asset。

**正式 release 應一律包含正確的 `checksums.txt`。**

* * *

## GitHub Release workflow

`.github/workflows/release.yml` 負責建置原生 binaries 並建立 GitHub Release。

觸發條件：

```yaml
on:
  push:
    tags:
      - 'v*'
```

也就是推送 `v1.0.0` 這類 tag 時觸發。

### 工作流程階段

| Job | 用途 |
|---|---|
| `test` | release 前執行 Rust tests |
| `build` | 針對 matrix targets 建置 binaries |
| `release` | 建立 GitHub Release 並上傳 assets |

### Build matrix

目前 matrix 應涵蓋：

```yaml
strategy:
  matrix:
    include:
      - target: x86_64-unknown-linux-gnu
        os: ubuntu-latest
        suffix: linux-x64
      - target: aarch64-unknown-linux-gnu
        os: ubuntu-latest
        suffix: linux-arm64
      - target: aarch64-pc-windows-msvc
        os: windows-latest
        suffix: win-arm64
      - target: x86_64-apple-darwin
        os: macos-latest
        suffix: darwin-x64
      - target: aarch64-apple-darwin
        os: macos-latest
        suffix: darwin-arm64
```

### Linux ARM64 cross compile

GitHub hosted Ubuntu runner 預設不一定具備 ARM64 GNU linker。

因此 workflow 需要：

```sh
sudo apt-get update && sudo apt-get install -y gcc-aarch64-linux-gnu
```

並指定 Cargo linker：

```yaml
env:
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER: aarch64-linux-gnu-gcc
```

如果缺少這段，常見錯誤會發生在 link 階段，而不是 Rust compile 階段。

### macOS Intel

macOS Intel target 是：

```text
x86_64-apple-darwin
```

對應 asset：

```text
mcp-cli-darwin-x64
```

使用者口語中的 `macos-intell` 應解讀為 macOS Intel。

### Windows ARM64

Windows ARM64 target 是：

```text
aarch64-pc-windows-msvc
```

對應 asset：

```text
mcp-cli-win-arm64.exe
```

Windows asset 必須包含 `.exe`，否則 npm wrapper 下載後與本機執行路徑不一致。

### Prepare asset

非 Windows 平台：

```sh
cp target/${{ matrix.target }}/release/mcp-cli dist/mcp-cli-${{ matrix.suffix }}
```

Windows 平台：

```sh
cp target/${{ matrix.target }}/release/mcp-cli.exe dist/mcp-cli-${{ matrix.suffix }}.exe
```

這段命名必須與 `scripts/install.js` 的 asset mapping 完全一致。

* * *

## npm trusted publishing workflow

`.github/workflows/npm-publish.yml` 負責發布 npm package。

典型內容：

```yaml
name: Publish npm package

on:
  release:
    types: [published]

jobs:
  publish:
    if: ${{ !github.event.release.prerelease }}
    runs-on: ubuntu-latest
    permissions:
      contents: read
      id-token: write
    steps:
      - uses: actions/checkout@v6

      - uses: actions/setup-node@v6
        with:
          node-version: 24
          registry-url: https://registry.npmjs.org

      - name: Publish to npm
        run: npm publish --access public --provenance
```

### 為什麼使用 `release.published`

npm package 安裝時需要 GitHub Release assets 已存在。

如果 npm package 先發布，而 release assets 尚未完成，使用者安裝會失敗。

所以 npm publish 應在 GitHub Release published 後執行。

### OIDC 權限

trusted publishing 需要：

```yaml
permissions:
  id-token: write
```

如果缺少此權限，npm 無法透過 GitHub OIDC 驗證 workflow 身分。

### `--provenance`

`--provenance` 會讓 npm 發布 supply chain provenance metadata。

命令：

```sh
npm publish --access public --provenance
```

### npm registry 端設定

GitHub workflow 只是其中一半。npm registry 端也必須設定 trusted publisher。

需要在 npm package settings 設定：

| 欄位 | 值 |
|---|---|
| Package | `@patrick85081/mcp-cli` |
| Publisher type | GitHub Actions |
| Repository | `iisi-2209026/mcp-cli` |
| Workflow | `.github/workflows/npm-publish.yml` |

如果 npm package 尚未建立，需依 npm 當前介面設定 trusted publishing。npm 介面可能變動，實際欄位以 npm 官方介面為準。

* * *

## 版本管理策略

### Rust crate version 與 npm version

本專案應保持：

```text
Cargo.toml version == package.json version == Git tag without leading v
```

範例：

```text
Cargo.toml: 1.0.0
package.json: 1.0.0
tag: v1.0.0
```

若版本不一致，會出現以下風險：

| 不一致情境 | 風險 |
|---|---|
| `package.json` 較新但 release tag 不存在 | npm install 404 |
| release tag 存在但 asset 是舊 binary | 使用者安裝到錯誤版本 |
| `Cargo.toml` 版本與 tag 不一致 | `mcp-cli --version` 與 npm version 不一致 |
| npm version 已發布但 GitHub Release 重建 | npm 使用者可能取得不同 binary，需避免 |

### SemVer 原則

建議遵守 SemVer：

| 版本類型 | 使用時機 |
|---|---|
| patch | bug fix、build fix、文件修正且需要重新發布 |
| minor | 新功能但相容既有 CLI 行為 |
| major | 破壞性 CLI 行為、config 格式、輸出格式變更 |

CLI 工具特別要注意 stdout 格式。若 stdout JSON 或命令輸出格式會被 script 依賴，變更可能是 breaking change。

* * *

## 正常發布流程

使用：

```sh
make release VERSION=1.0.0
```

此命令會呼叫：

```sh
./scripts/release.sh 1.0.0
```

release script 應執行：

1. 檢查 VERSION 是否存在。
2. 檢查 VERSION 是否符合 `X.Y.Z`。
3. 檢查目前 branch 是否為 `main`。
4. 檢查 worktree 是否乾淨。
5. 檢查 tag 是否已存在。
6. 更新 `Cargo.toml`。
7. 更新 `package.json`。
8. 執行 `cargo check` 更新 `Cargo.lock`。
9. 執行格式檢查、clippy、tests。
10. commit version bump。
11. 建立 annotated tag。
12. push `main`。
13. push tag。

### 發布後自動流程

```text
git push origin v1.0.0
        |
        v
release.yml starts
        |
        v
build binaries
        |
        v
create GitHub Release
        |
        v
release.published event
        |
        v
npm-publish.yml starts
        |
        v
npm publish --provenance
```

* * *

## 重新發布既有 GitHub Release

只有在以下情境才應重新發布同一個 GitHub tag：

| 情境 | 是否合理 |
|---|---|
| GitHub Release assets 缺失 | 合理 |
| checksums.txt 錯誤 | 合理 |
| workflow matrix 漏平台 | 合理 |
| npm package 已發布但想改內容 | 不合理，npm 不可覆寫 |
| 想修正程式 bug | 應發布新版本 |

重新發布 `v1.0.0` 的命令：

```sh
gh release delete v1.0.0 --cleanup-tag --yes
git tag -d v1.0.0 2>/dev/null || true
git tag -a v1.0.0 -m "Release v1.0.0"
git push origin v1.0.0
```

### 風險

**如果 npm 已發布同版本，重新建立 GitHub Release 可能讓同一 npm version 下載到不同 binary。**

這會降低可重現性。除非該 npm version 尚未發布，否則更安全的做法是發布新 patch version。

* * *

## npm 不可變性

npm registry 的版本一旦發布，不可覆寫。

例如：

```text
@patrick85081/mcp-cli@1.0.0
```

一旦發布成功，再次執行：

```sh
npm publish --access public --provenance
```

會失敗，因為 `1.0.0` 已存在。

這不是 GitHub Actions 問題，也不是 authentication 問題，而是 npm registry 的版本不可變規則。

解法：

```text
1.0.0 -> 1.0.1
```

並重新建立：

```text
v1.0.1
```

* * *

## 本機檢查 npm package

發布前可檢查 package 內容：

```sh
npm pack --dry-run
```

應確認輸出包含：

```text
package.json
bin/mcp-cli.js
scripts/install.js
README.md
LICENSE
```

不應包含：

```text
vendor/mcp-cli
vendor/mcp-cli.exe
target/
node_modules/
```

### 為什麼不要把 binary 放進 npm package

如果把所有平台 binaries 都放進 npm package：

| 問題 | 說明 |
|---|---|
| package 變大 | 每個使用者只需要一個平台的 binary |
| 發布流程複雜 | 每次 release 都要把所有 binaries 打包進 npm tarball |
| 平台擴充成本高 | 新平台會增加 tarball 大小 |
| npm cache 成本高 | CI 與使用者下載都較慢 |

目前採取 postinstall 下載策略，可以讓 npm package 保持很小。

* * *

## 使用者安裝方式

Global install：

```sh
npm install -g @patrick85081/mcp-cli
```

指定版本：

```sh
npm install -g @patrick85081/mcp-cli@1.0.0
```

使用 `npx`：

```sh
npx @patrick85081/mcp-cli --version
```

如果 `npx` 每次都重新建立暫存環境，可能會每次觸發 postinstall 下載 binary。

* * *

## 常見失敗模式

### `postinstall` 404

原因：GitHub Release asset 不存在。

檢查：

```text
https://github.com/iisi-2209026/mcp-cli/releases/download/v<VERSION>/<ASSET>
```

常見原因：

| 原因 | 修正 |
|---|---|
| tag 不存在 | 建立正確 tag |
| release workflow 失敗 | 修正 workflow 後重新跑 release |
| asset name 不一致 | 同步 `release.yml` 與 `scripts/install.js` |
| npm version 與 GitHub tag 不一致 | 修正版本對齊 |

### `Checksum mismatch`

原因：下載的 binary SHA-256 與 `checksums.txt` 不一致。

可能原因：

| 原因 | 修正 |
|---|---|
| release asset 被替換但 checksum 未更新 | 重新產生 `checksums.txt` |
| 上傳了錯誤檔案 | 重新建置 release |
| asset name 對到錯平台 binary | 檢查 build matrix suffix |

### `mcp-cli native binary is not installed`

原因：`vendor/` 內沒有 binary。

常見原因：

```sh
npm install --ignore-scripts
```

修正：

```sh
npm rebuild @patrick85081/mcp-cli
```

或重新安裝：

```sh
npm uninstall -g @patrick85081/mcp-cli
npm install -g @patrick85081/mcp-cli
```

### Trusted publishing authentication error

檢查：

1. npm package settings 是否已設定 trusted publisher。
2. repository 是否正確。
3. workflow path 是否正確。
4. workflow 是否有 `id-token: write`。
5. 是否使用 `npm publish --provenance`。
6. 是否從 npm 信任的 event、branch 或 environment 執行。

### npm publish version already exists

原因：npm version 已發布。

修正：發布新版本。

```sh
make release VERSION=1.0.1
```

### Linux ARM64 build failed

如果錯誤發生在 linker：

```text
linking with `cc` failed
```

或找不到 aarch64 linker，確認 workflow 有：

```sh
sudo apt-get update && sudo apt-get install -y gcc-aarch64-linux-gnu
```

以及：

```yaml
env:
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER: aarch64-linux-gnu-gcc
```

### Windows ARM64 asset 找不到

確認 release asset 是：

```text
mcp-cli-win-arm64.exe
```

而不是：

```text
mcp-cli-windows-arm64.exe
mcp-cli-win-arm64
mcp-cli-aarch64-windows.exe
```

asset name 必須與 `scripts/install.js` 完全一致。

* * *

## 新增平台支援流程

若要新增新平台，需同步修改多處。

### 1. 新增 release matrix

在 `.github/workflows/release.yml` 加入 target：

```yaml
- target: <rust-target-triple>
  os: <github-runner>
  suffix: <asset-suffix>
```

### 2. 確認 Rust target 可建置

可能需要：

```sh
rustup target add <target>
```

GitHub workflow 使用 `actions-rust-lang/setup-rust-toolchain` 時可指定 target。

### 3. 確認 linker

Cross compile 通常需要 linker。

Linux ARM64 需要 `gcc-aarch64-linux-gnu` 是典型範例。

### 4. 更新 asset upload list

release job 的 files list 必須包含新 asset。

### 5. 更新 `scripts/install.js`

新增 `process.platform` 與 `process.arch` mapping。

### 6. 更新文件

更新本文件的支援平台表格。

### 7. 驗證 release asset URL

確認 URL 可下載：

```text
https://github.com/iisi-2209026/mcp-cli/releases/download/v<VERSION>/<ASSET>
```

* * *

## 安全與供應鏈注意事項

### Trusted publishing

使用 trusted publishing 可以避免長期保存 `NPM_TOKEN`。

優點：

| 優點 | 說明 |
|---|---|
| 不需 secret token | 降低 token 洩漏風險 |
| OIDC 短期憑證 | GitHub Actions 執行時即時取得 |
| provenance | 可追蹤 package 由哪個 workflow 發布 |

### Checksum

checksum 可降低下載錯誤或 asset 被錯置的風險。

但 checksum 與 binary 同在同一個 GitHub Release，不能取代完整的簽章機制。

若未來需要更高安全性，可考慮：

| 機制 | 用途 |
|---|---|
| Sigstore/cosign | 簽署 release assets |
| SLSA provenance | 建置來源與過程證明 |
| npm provenance | npm package 發布來源證明 |

### 不應在 postinstall 執行任意建置

避免在使用者機器上：

```sh
cargo build --release
```

原因：

| 問題 | 說明 |
|---|---|
| 需要 Rust toolchain | 增加安裝門檻 |
| 安裝時間長 | 使用者體驗差 |
| build 環境不一致 | 難以保證可重現 |
| CI dependency 複雜 | npm install 可能變慢或失敗 |

* * *

## 維護規則

### 修改 binary asset name 時

必須同步修改：

1. `.github/workflows/release.yml`
2. `scripts/install.js`
3. `install.sh`
4. `docs/npm-publishing.md`
5. `README.md`，如果有提到 asset 或平台支援

### 修改 npm package name 時

必須同步修改：

1. `package.json` 的 `name`
2. `.github/workflows/npm-publish.yml`，如有 package-specific 設定
3. npm registry trusted publisher 設定
4. README 安裝命令
5. 本文件

### 修改 repository owner/name 時

必須同步修改：

1. `package.json` repository、homepage、bugs
2. `scripts/install.js` 的預設 repo
3. `install.sh` 的 `GITHUB_REPO`
4. npm trusted publisher 設定
5. README 安裝連結
6. 本文件

### 修改版本發布方式時

必須確認：

1. `Cargo.toml` version 是否更新。
2. `package.json` version 是否更新。
3. `Cargo.lock` 是否反映 crate version。
4. tag 是否使用 `v<VERSION>` 格式。
5. npm wrapper 是否仍能推導正確 GitHub Release tag。

* * *

## Release 前檢查清單

發布前應確認：

```text
[ ] worktree 乾淨
[ ] 目前 branch 是 main
[ ] Cargo.toml version 正確
[ ] package.json version 正確
[ ] README 安裝說明正確
[ ] release.yml matrix 包含預期平台
[ ] scripts/install.js 支援所有 release assets
[ ] npm trusted publisher 已設定
[ ] npm package version 尚未存在
[ ] npm pack --dry-run 內容合理
```

* * *

## Release 後檢查清單

發布後應確認：

```text
[ ] GitHub Actions release workflow 成功
[ ] GitHub Release 已建立
[ ] release assets 完整
[ ] checksums.txt 存在
[ ] npm-publish workflow 成功
[ ] npm registry 可看到新版本
[ ] npm install -g @patrick85081/mcp-cli@<VERSION> 可下載 binary
[ ] mcp-cli --version 顯示預期版本
```

如果沒有明確要求，不應在維護流程中自動執行這些驗證；此清單是發布操作者使用。

* * *

## 目前已知限制

| 限制 | 說明 |
|---|---|
| 不支援 Windows x64 | 目前 installer 未 mapping `win32/x64` |
| 不支援 Linux musl | 目前 release asset 使用 GNU target |
| checksum 不存在時會跳過驗證 | 為安裝彈性保留，但正式 release 應提供 checksum |
| npm version 不可覆寫 | 發布錯誤需 bump patch version |
| postinstall 依賴 GitHub Release 可用性 | GitHub Release asset 缺失會導致安裝失敗 |

* * *

## 建議後續改善

可考慮的改善項目：

1. 新增 Windows x64 support。
2. 新增 Linux musl binaries。
3. 將 checksum 缺失改為 hard failure。
4. 加入 release asset 簽章。
5. 加入 npm package smoke test workflow。
6. 在 release 完成後測試每個 asset URL。
7. 將平台 mapping 抽出成單一資料表，減少 release workflow 與 installer 命名不一致風險。
8. 在 README 加入 npm 安裝疑難排解。

* * *

## 命令速查

正常發布：

```sh
make release VERSION=1.0.0
```

本機檢查 npm package 內容：

```sh
npm pack --dry-run
```

重新下載 npm binary：

```sh
npm rebuild @patrick85081/mcp-cli
```

重新發布 GitHub Release tag：

```sh
gh release delete v1.0.0 --cleanup-tag --yes
git tag -d v1.0.0 2>/dev/null || true
git tag -a v1.0.0 -m "Release v1.0.0"
git push origin v1.0.0
```

指定下載 repository 與版本測試安裝：

```sh
MCP_CLI_REPO=iisi-2209026/mcp-cli MCP_CLI_VERSION=v1.0.0 npm install -g @patrick85081/mcp-cli
```

查詢 npm package：

```sh
npm view @patrick85081/mcp-cli
```

查詢特定 npm version：

```sh
npm view @patrick85081/mcp-cli@1.0.0
```

* * *

## 首次部署與 trusted publishing 啟用順序

**首次部署通常需要先用傳統 npm publish 建立 package，然後才能在 npm package settings 設定 trusted publishing。**

原因是 npm 官方 trusted publishing 文件的主要設定入口是：

```text
npmjs.com -> Packages -> YOUR_PACKAGE -> Settings -> Trusted publishing
```

也就是說，透過網站設定 trusted publisher 時，通常必須先有 package 頁面與 package settings。若 package 尚未存在，通常沒有可操作的 package settings。

npm CLI 也提供 `npm trust github` 來建立 trusted publisher 設定，但實務上仍應以目前 npm registry 對該 package 狀態的支援為準。若 package 尚未建立，最穩定、最可預期的流程仍是先手動發布第一版，建立 package 後再設定 trusted publishing。

* * *

## 首次發布的可靠流程

首次發布 `@patrick85081/mcp-cli` 時，建議採用以下順序。

### 1. 確認 package name 可用

先查詢 package 是否已存在：

```sh
npm view @patrick85081/mcp-cli
```

如果 package 不存在，npm 通常會回傳 404 類型錯誤。

這代表 package name 尚未被發布，但不代表 scope 權限一定正確。還需要確認目前登入的 npm 帳號具有發布 `@willh` scope package 的權限。

### 2. 登入 npm

```sh
npm login
```

確認目前登入身分：

```sh
npm whoami
```

### 3. 檢查 package 內容

```sh
npm pack --dry-run
```

首次發布前，必須確認 package 只包含預期內容：

```text
package.json
bin/mcp-cli.js
scripts/install.js
README.md
LICENSE
```

不得包含：

```text
vendor/
target/
node_modules/
dist/
.env
```

### 4. 確認 GitHub Release assets 已存在

因為 npm package 的 `postinstall` 會下載 GitHub Release binary，所以首次 npm publish 前，對應版本的 GitHub Release assets 必須已存在。

例如要發布：

```text
@patrick85081/mcp-cli@1.0.0
```

就必須先有：

```text
https://github.com/iisi-2209026/mcp-cli/releases/download/v1.0.0/
```

且至少包含目前支援平台的 assets：

```text
mcp-cli-linux-x64
mcp-cli-linux-arm64
mcp-cli-darwin-x64
mcp-cli-darwin-arm64
mcp-cli-win-arm64.exe
checksums.txt
```

如果先發布 npm package，但 GitHub Release assets 尚未存在，使用者安裝時會在 `postinstall` 階段失敗。

### 5. 手動首次發布 npm package

首次建立 scoped public package 時使用：

```sh
npm publish --access public
```

若要同時產生 provenance，可在支援的環境使用：

```sh
npm publish --access public --provenance
```

但需注意：本機手動發布通常不是 GitHub Actions OIDC 環境，`--provenance` 不一定可用。首次手動發布的重點是建立 package 與 package settings。

若 npm 帳號啟用 2FA，npm 可能要求 OTP：

```sh
npm publish --access public --otp <OTP>
```

或互動式輸入 OTP。

### 6. 到 npm package settings 設定 trusted publisher

首次發布成功後，開啟 npm package settings：

```text
npmjs.com -> Packages -> @patrick85081/mcp-cli -> Settings -> Trusted publishing
```

設定 GitHub Actions trusted publisher。

欄位應如下：

| npm 欄位 | 本專案設定 |
|---|---|
| Provider | GitHub Actions |
| Organization or user | `iisi-2209026` |
| Repository | `mcp-cli` |
| Workflow filename | `npm-publish.yml` |
| Environment name | 空白，除非 workflow 使用 GitHub Environment |
| Allowed actions | `npm publish` |

注意：npm 官方文件要求 GitHub Actions 的 workflow 欄位填的是 workflow filename，不是完整路徑。

正確：

```text
npm-publish.yml
```

不是：

```text
.github/workflows/npm-publish.yml
```

workflow 檔案本身仍必須存在於 repository 的：

```text
.github/workflows/npm-publish.yml
```

### 7. 確認 workflow 權限

`.github/workflows/npm-publish.yml` 必須包含：

```yaml
permissions:
  contents: read
  id-token: write
```

`id-token: write` 是 OIDC trusted publishing 必要條件。

### 8. 確認 `package.json` repository 欄位

npm 官方文件指出，從 GitHub trusted publishing 時，`package.json` 的 `repository.url` 必須與 GitHub repository 精確對應。

本專案應為：

```json
{
  "repository": {
    "type": "git",
    "url": "git+https://github.com/iisi-2209026/mcp-cli.git"
  }
}
```

如果 repository 欄位指向 fork 或錯誤 owner，trusted publishing 可能失敗。

### 9. 後續版本改用 GitHub Actions 發布

trusted publisher 設定完成後，後續版本就不需要 `NPM_TOKEN`，也不需要手動 `npm publish`。

正常流程改為：

```sh
make release VERSION=1.0.1
```

流程會是：

```text
push tag v1.0.1
        |
        v
release.yml 建置 binaries 並建立 GitHub Release
        |
        v
GitHub Release published
        |
        v
npm-publish.yml 執行 npm publish
        |
        v
npm registry 發布 @patrick85081/mcp-cli@1.0.1
```

* * *

## 可選流程：使用 `npm trust github` 設定 trusted publisher

npm CLI 提供 `npm trust github`，可用 CLI 管理 trusted publisher 設定。

基本格式：

```sh
npm trust github @patrick85081/mcp-cli \
  --repo iisi-2209026/mcp-cli \
  --file npm-publish.yml \
  --allow-publish
```

檢查現有 trusted publisher：

```sh
npm trust list @patrick85081/mcp-cli
```

移除 trusted publisher 時，先查出 trust id：

```sh
npm trust list @patrick85081/mcp-cli
```

再撤銷：

```sh
npm trust revoke @patrick85081/mcp-cli --id <trust-id>
```

限制與注意事項：

1. 每個 package 目前只能有一個 trusted publisher 設定。
2. 建立 trust relationship 時至少要指定 `--allow-publish` 或 `--allow-stage-publish`。
3. `--file` 是 workflow filename，例如 `npm-publish.yml`，不是完整路徑。
4. 如果 repository 未透過 `--repo` 指定，npm 可能會從 `package.json` 的 `repository.url` 推導。
5. 對於尚未首次發布的 package，是否能直接用 `npm trust github` 完成首發前設定，應以當下 npm CLI 與 registry 實際行為為準；本專案的可靠操作流程仍建議先手動首次發布。

* * *

## 首次發布後的安全收斂

首次手動發布完成、trusted publishing 驗證成功後，應降低傳統 token 風險。

建議：

1. 移除未使用的 npm automation token。
2. 在 npm package settings 的 publishing access 中限制 token-based publishing。
3. 若團隊流程允許，可要求 2FA 並 disallow tokens。
4. 僅保留 trusted publisher 作為 CI 發布路徑。
5. 保護 GitHub tag 建立權限，避免任意人推送 `v*` tag 觸發發布。
6. 可使用 GitHub Environment 增加人工 approval，再將 npm trusted publisher 的 Environment name 設為相同名稱。

若啟用 GitHub Environment，workflow job 也要指定：

```yaml
jobs:
  publish:
    environment: npm-publish
```

npm trusted publisher 設定中的 Environment name 也要填：

```text
npm-publish
```

兩邊名稱必須一致。

* * *

## 首次部署檢查清單

```text
[ ] npm 帳號已登入
[ ] npm 帳號具備 @willh scope 發布權限
[ ] @patrick85081/mcp-cli 尚未被其他 package 佔用
[ ] package.json name 是 @patrick85081/mcp-cli
[ ] package.json version 是要首發的版本
[ ] package.json repository.url 指向 iisi-2209026/mcp-cli
[ ] npm pack --dry-run 內容正確
[ ] GitHub Release v<VERSION> 已存在
[ ] GitHub Release assets 完整
[ ] checksums.txt 存在且正確
[ ] 已手動執行 npm publish --access public
[ ] npm package settings 可開啟
[ ] trusted publisher 已設定 GitHub Actions
[ ] Workflow filename 填 npm-publish.yml
[ ] Allowed actions 包含 npm publish
[ ] .github/workflows/npm-publish.yml 有 id-token: write
[ ] 後續版本可由 GitHub Actions 發布
```

* * *

## 本專案首次發布的建議實作順序

若 `@patrick85081/mcp-cli` 尚未存在於 npm registry，建議採用以下順序：

1. 先確保 `main` 上的 wrapper、release workflow、npm publish workflow 都已合併。
2. 建立並完成 GitHub Release，例如 `v1.0.0`。
3. 確認 `v1.0.0` 的所有 release assets 都存在。
4. 本機執行 `npm pack --dry-run`。
5. 本機手動執行 `npm publish --access public` 建立 `@patrick85081/mcp-cli`。
6. 到 npm package settings 設定 trusted publisher。
7. 後續從 `v1.0.1` 起改用 GitHub Actions trusted publishing。

如果 `v1.0.0` 已經手動發布到 npm，GitHub Actions 後續不應再嘗試發布 `1.0.0`，因為 npm version 不可覆寫。

此時應發布下一版：

```sh
make release VERSION=1.0.1
```

* * *

## 重要更正：workflow filename 不是完整路徑

npm trusted publisher 設定 GitHub Actions 時，npm 官方文件要求填寫 workflow filename。

本專案的正確值是：

```text
npm-publish.yml
```

不是：

```text
.github/workflows/npm-publish.yml
```

完整路徑只用於 repository 內檔案位置說明：

```text
.github/workflows/npm-publish.yml
```
