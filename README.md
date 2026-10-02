
# MoliWhisper

通过利用豆包 Web 版的语音识别能力，在 macOS 上实现全局语音输入：按下右 `⌥ Option` 键开始/停止语音识别，识别结果自动复制到剪贴板并粘贴到当前光标所在的输入框。

> 本仓库 fork 自 [lilong7676/doubao-murmur](https://github.com/lilong7676/doubao-murmur)，现独立维护，只保留 macOS 版。原项目的 Windows / Linux 版本可在原仓库或本仓库的 Git 历史中找到。

## 组成

语音识别有三种来源（设置里切换）：豆包网页版（需登录）、豆包输入法（免登录）、千问（阿里云百炼，需要 API Key）。文字整理有两种：豆包输入法、任意 OpenAI 兼容接口。

### 千问语音识别

在[阿里云百炼](https://bailian.console.aliyun.com/)创建 API Key，然后在 MoliWhisper 设置里选「千问（阿里云）」，填入 Key，点「保存并测试」。模型默认 `qwen-audio-3.0-asr-flash-streaming`，地址默认 `wss://maas.qwencloudapi.com/api-ws/v1/inference`，一般不用改。Key 只保存在本机的 0600 配置文件里。

<p align="center">
  <img src="docs/screenshots/overlay_pannel.png" width="500" alt="语音识别悬浮窗">
</p>

## 免责声明

- **本项目仅供个人学习和研究使用**，不得用于任何商业用途。
- 本项目通过内嵌 WKWebView 加载豆包（doubao.com）网页版来调用其语音识别功能，**并非官方提供的 API 或 SDK**。豆包的页面结构、接口随时可能变化，届时本项目可能无法正常工作。
- 使用本项目前，你需要拥有一个有效的豆包账号并自行完成登录。
- 本项目不会收集、存储或上传你的任何数据（包括语音数据和识别结果），所有处理均在本地完成，语音数据由豆包服务端处理。
- 使用本项目所产生的一切后果由使用者自行承担，作者不对因使用本项目而导致的任何损失或问题负责。
- 如果本项目侵犯了相关方的权益，请联系作者删除。

## 核心原理

首次使用时，应用通过内嵌 WebView 加载豆包网页版完成登录，提取认证凭证（Cookie、设备标识等）保存到本地后立即销毁 WebView 释放资源。

后续使用时无需再加载网页。应用直接使用本地保存的凭证，通过原生 WebSocket 连接豆包的流式语音识别服务，将麦克风采集的音频实时发送到服务端，接收识别结果后自动粘贴到当前输入框。

当凭证过期时，应用会自动检测并提示重新登录，登录后再次提取凭证并销毁 WebView，如此循环。

## 使用方式

### 安装

要求 macOS 13.0+（Apple Silicon）。master 每次推送、CI 通过后都会发布一个新版本（`X.Y.N`，N 是提交数，保留最近 10 个），在 [Releases](https://github.com/MoliDuo/MoliWhisper/releases)。

第一次安装最新版到「应用程序」文件夹，需要 [gh](https://cli.github.com)：

```bash
./scripts/update.sh
```

用 `gh` 下载不会被 Gatekeeper 拦；从网页下载 dmg 的话，首次打开的办法见发布说明。也可以从源码构建安装：

```bash
./scripts/install.sh
```

### 自动更新

装好之后应用会自己更新，做法和 MoliSwitch 一样：

- 启动后和之后每小时检查一次 GitHub 上的最新版本，没有新版本或网络出错时不打扰。
- 发现新版本时弹窗询问，选「安装并重启」才下载、替换并重启；选「稍后」则本次运行不再提醒这个版本。
- 菜单栏的「检查更新…」和设置页「通用」里的按钮可以手动检查，会明确告诉你「已是最新版本」或失败原因。
- 从 dmg 里直接运行（`/Volumes/…`）或被 macOS 放在只读临时位置运行时无法自我替换，会提示先拖进「应用程序」。
- 每个版本都用同一张证书签名，更新后辅助功能和麦克风授权保留。

更新清单是 `https://github.com/MoliDuo/MoliWhisper/releases/latest/download/latest.json`，其中的下载地址指向该版本自己的更新包，不用 `latest`。更新包用单独的更新密钥签名（公钥在 `tauri.conf.json`），应用只安装签名有效、且签名里的版本号与清单一致的包。

### 首次使用

1. **授予辅助功能权限**：首次启动时，系统会提示授予辅助功能权限（系统设置 → 隐私与安全性 → 辅助功能），这是监听全局快捷键所必需的。
2. **授予麦克风权限**：首次语音输入时，系统会提示授予麦克风权限。
3. **登录豆包**：点击菜单栏图标，选择「登录豆包」，在弹出的窗口中完成登录。登录成功后窗口会自动关闭。

### 快捷键

| 操作 | 按键 |
|------|------|
| 开始 / 停止 | 右 `⌥ Option` |
| 取消 | `ESC` |

取消是指放弃本次识别，不复制也不粘贴。

### 使用流程

<img src="docs/screenshots/menu_bar.png" width="240" alt="菜单栏">

1. 确保菜单栏显示「已登录」状态
2. 将光标定位到任意输入框
3. 按下右 `⌥`，屏幕顶部出现悬浮窗，开始说话
4. 悬浮窗中会实时显示识别到的文字
5. 再次按下右 `⌥` 结束识别，文字会自动复制到剪贴板并粘贴到输入框
6. 如果想取消，按 `ESC` 即可

点击菜单中的「使用帮助」可查看快捷键和使用说明：

<img src="docs/screenshots/help_pannel.png" width="400" alt="使用帮助">

## 开发

> 正在用 Tauri 2 + Rust 重写 2.0（macOS + Windows）。旧的 Swift 版暂时放在 [`legacy/swift/`](legacy/swift)，新版能日常使用后删除。

### 目录

| 路径 | 内容 |
|------|------|
| [`crates/moli-core/`](crates/moli-core) | 纯 Rust 核心：豆包网页版与输入法两套 ASR 客户端、千问（DashScope）客户端、文字整理（豆包 / OpenAI 兼容）、会话逻辑、音频处理。不依赖 Tauri，任何平台都能 `cargo test` |
| [`src-tauri/`](src-tauri) | Tauri 应用：托盘、窗口、平台相关代码（热键、粘贴、悬浮窗、权限） |
| [`ui/`](ui) | 悬浮窗和设置页，Vite + 纯 TypeScript |
| [`scripts/`](scripts) | 构建、安装、调试脚本 |
| [`docs/`](docs) | 豆包接口的逆向笔记和截图 |
| [`legacy/`](legacy) | 旧的 Swift 版和当时的设计文档，新版能日常使用后删除 |

### 环境要求

- Rust stable、Node.js 22+、pnpm
- macOS 13.0+（Xcode Command Line Tools）或 Windows 10/11（WebView2）

### 常用命令

```bash
# 构建带签名的 debug .app 并启动（测试热键、粘贴、麦克风用这个）
./scripts/dev-app.sh

# 只调界面：tauri dev（权限会记在终端名下）
./scripts/dev.sh

# 构建 release 版，安装到 /Applications 并启动
./scripts/install.sh

# 从 GitHub 装最新发布的版本（或指定标签）
./scripts/update.sh [v2.0.97]

# 改主次版本号（补丁号由 CI 按提交数填）
./scripts/set-version.sh 2.1.0

# 看日志 / 结束进程 / 重置系统授权
./scripts/logs.sh
./scripts/kill.sh
./scripts/reset-tcc.sh

# 核心库测试
cargo test -p moli-core

# 真实接口探针（凭证默认读旧版 app 保存的文件，或用 MOLI_CREDS 指定）
./scripts/asr-probe.sh --wav crates/moli-core/fixtures/zh_short.wav --repeat 20
```

本地构建会自动找钥匙串里的 Apple Development 证书签名，这样重新构建后辅助功能和麦克风授权不会丢。CI 用同一张证书（仓库 Secrets 里的 `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD`）签发布包，所以本地构建和下载的版本共用同一份授权。

更新包另用一把更新密钥签名：`./scripts/setup-updater-keys.sh` 在本机生成一次，公钥写进 `tauri.conf.json`，私钥和密码存进 Secrets 的 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。没有它 CI 不会发布。私钥丢了，已安装的版本就再也收不到自动更新，务必备份。

## License

[MIT](LICENSE)
