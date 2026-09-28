
# MoliWhisper

通过利用豆包 Web 版的语音识别能力，在 macOS 上实现全局语音输入：按下右 `⌥ Option` 键开始/停止语音识别，识别结果自动复制到剪贴板并粘贴到当前光标所在的输入框。

> 本仓库 fork 自 [lilong7676/doubao-murmur](https://github.com/lilong7676/doubao-murmur)，现独立维护，只保留 macOS 版。原项目的 Windows / Linux 版本可在原仓库或本仓库的 Git 历史中找到。

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

要求 macOS 13.0+。从源码构建并安装到「应用程序」文件夹：

```bash
./scripts/install.sh
```

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
| [`crates/moli-core/`](crates/moli-core) | 纯 Rust 核心：豆包网页版与输入法两套 ASR 客户端、会话逻辑、音频处理。不依赖 Tauri，任何平台都能 `cargo test` |
| [`src-tauri/`](src-tauri) | Tauri 应用：托盘、窗口、平台相关代码（热键、粘贴、悬浮窗、权限） |
| [`ui/`](ui) | 悬浮窗和设置页，Vite + 纯 TypeScript |
| [`scripts/`](scripts) | 构建、安装、调试脚本 |

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

# 看日志 / 结束进程 / 重置系统授权
./scripts/logs.sh
./scripts/kill.sh
./scripts/reset-tcc.sh

# 核心库测试
cargo test -p moli-core

# 真实接口探针（凭证默认读旧版 app 保存的文件，或用 MOLI_CREDS 指定）
./scripts/asr-probe.sh --wav crates/moli-core/fixtures/zh_short.wav --repeat 20
```

本地构建会自动找钥匙串里的 Apple Development 证书签名，这样重新构建后辅助功能和麦克风授权不会丢。

## License

[MIT](LICENSE)
