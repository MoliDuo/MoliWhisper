# 架构

Moli Whisper 是 macOS（Apple 芯片，13+）菜单栏上的语音输入工具。按热键开始说话，识别结果粘贴到当前光标处。没有服务端，也没有账号：应用直接连语音识别和文字整理的服务。

## 组成

| 部分 | 做什么 |
| --- | --- |
| `crates/moli-core` | 纯 Rust 核心，不依赖 Tauri，大部分测试在这里。<br>包括：千问实时语音识别客户端（DashScope WebSocket）、DeepSeek 整理（OpenAI 兼容的流式 `/chat/completions`）、会话状态机、音频重采样和处理、热键匹配、配置结构。 |
| `src-tauri` | Tauri 应用：托盘菜单、悬浮窗和设置窗口、更新、开机启动、单实例。<br>macOS 专属的代码在 `src/platform/macos`：全局热键监听、模拟粘贴、悬浮窗层级、权限检查。 |
| `ui` | 悬浮窗和设置页，Vite + 纯 TypeScript，不用框架。 |
| `scripts/release` | 发版流水线用的 Node 脚本：生成并核对 `latest.json`，用公钥校验更新签名，生成 `SHA256SUMS` 和中文发布说明。 |

一次听写的流程：

1. 热键按下，从麦克风采集音频。
2. 音频重采样到 16 kHz，经 WebSocket 推给千问。
3. 悬浮窗实时显示中间结果。
4. 结束后拿到最终文本。开了自动整理，就交给 DeepSeek 流式改写，悬浮窗同时显示改写结果；超过 15 秒或出错就用原文。
5. 写进剪贴板，模拟 ⌘V 粘贴。

## 决定和理由

### 用 Tauri 2 + Rust

- **为什么**：上游 doubao-murmur 就是 Tauri。Rust 核心可以脱离界面单独测试，安装包也比 Electron 小得多（规范 010 不允许 Electron）。
- **怎么分层**：界面只有两个小页面，所以不用前端框架。

### 只支持 macOS arm64

- **为什么**：上游的 Windows、Linux 移植已经删掉（`954285f`）。
- **取舍**：全局热键和模拟粘贴都依赖 macOS 的辅助功能接口，维护多平台的成本不值得。
- **代码里还剩什么**：非 macOS 只保留能编译的空实现。

### 语音识别用千问，整理用 DeepSeek

- **怎么来的**：豆包输入法后端（要登录）换成了自建 ASR 服务，又换成了 DashScope 上的千问实时识别（`ba0052f`、`74f69d3`）。
- **为什么**：现在不用维护服务器，用户只需要填一个 API Key。
- **整理接口**：走 OpenAI 兼容接口，换模型只改地址和模型名。

### 不接统一登录（规范 008 例外）

- **为什么**：没有服务端，也没有需要保护的 Moli 数据。登录只会多一步，却保护不了什么。
- **用户的凭据**：只有自己填的第三方 API Key，存在 `~/Library/Application Support/com.moliduo.moliwhisper/config.json`（0600），不进钥匙串（规范 012）。
- **登记**：例外写在 `moli.yaml`。

### 更新用 tauri-plugin-updater（规范 007 的 7.2.1）

- **为什么用它**：这是 Tauri 官方的更新插件。更新包用 minisign（Ed25519）签名，公钥编译进应用（`tauri.conf.json` 的 `plugins.updater.pubkey`），`requireSignedVersion` 要求签名里的版本和清单一致。
- **为什么不用 Sparkle**：要另接 Objective-C 框架。现成的插件已经满足规范 007 的要求：固定更新源、只认签名、不强制更新。
- **更新源**：固定是 `releases/latest/download/latest.json`，清单里的下载地址指向具体版本（`/download/vX.Y.Z/`），不用 `latest`。
- **检查节奏和提示**：在 `src-tauri/src/updater.rs`，按规范 7.4 实现：
  - 约 10 秒后首查，之后每小时一次，后台检查没有更新时不出声。
  - 手动检查一定给结果。
  - 询问时按钮是「立即更新」和「稍后」。
  - 从磁盘映像或系统的临时位置运行时拒绝更新，并说明原因。

### 代码签名用共用的自签名证书

- **用哪张证书**：发布包用 Moli 系列共用的「Moli Self-Signed Code Signing」证书签名（组织密钥 `CODESIGN_P12_*`，SHA-1 指纹写在 `release.yml`），不公证。
- **为什么**：macOS 按签名身份记住辅助功能和麦克风授权。证书不变，更新后授权就保留。
- **这样做的代价**：第一次打开要在「隐私与安全性」里点「仍要打开」。
- **本地构建**：钥匙串里有这张证书就用它签，没有就退回 ad-hoc 签名。

### Bundle ID 不改（规范 001 例外）

- **规范怎么说**：按规范 001，Bundle ID 应该是 `com.moliduo.whisper`。
- **为什么不改**：已发布的版本用的是 `com.moliduo.moliwhisper`。改了相当于换了一个应用，所有用户要重新授权，配置目录也会变。
- **文件名**：`productName` 也保持 `MoliWhisper`，它决定 `.app` 和安装包的文件名，不带空格。
- **界面里的名字**：给人看的名字是 `Moli Whisper`，写在 `CFBundleDisplayName`、窗口标题和对话框里。

### 版本只写一处（规范 006 的 6.2.2）

- **写在哪**：`package.json` 的 `version`。
- **谁读它**：`tauri.conf.json` 的 `version` 指向它，应用、安装包和更新清单都从这里拿版本。
- **Cargo 里的版本**：Cargo 工作区的版本固定为 `0.0.0`，那些 crate 不发布。
- **怎么发版**：手动，改版本、合并 `chore(release): vX.Y.Z`、打标签推送，步骤见 `AGENTS.md`。
- **不再用的做法**：以前“每次推送按提交数出一个版本”，已经停用。

### CI 不用缓存

- **规定**：规范 004 的 4.6.7 不允许用 GitHub 缓存。
- **代价**：每次都完整编译 Rust，`check` 和 `build` 各要十几分钟。
- **为什么接受**：换来的是构建可复现，也不用担心缓存被污染。
