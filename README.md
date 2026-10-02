# MoliWhisper

macOS 上的全局语音输入：按下右 `⌥ Option` 开始说话，再按一次结束，识别结果自动粘贴到当前光标所在的输入框。

语音识别用阿里云百炼的千问实时语音识别，可选的「自动整理」（口语 → 书面语）用 DeepSeek。两项都只需要填一个 API Key，地址和模型都有默认值。

<p align="center">
  <img src="docs/screenshots/overlay_pannel.png" width="500" alt="语音识别悬浮窗">
</p>

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

装好之后应用会自己更新：

- 启动后和之后每小时检查一次 GitHub 上的最新版本，没有新版本或网络出错时不打扰。
- 发现新版本时弹窗询问，选「安装并重启」才下载、替换并重启；选「稍后」则本次运行不再提醒这个版本。
- 菜单栏的「检查更新…」和设置页「通用」里的按钮可以手动检查，会明确告诉你「已是最新版本」或失败原因。
- 从 dmg 里直接运行（`/Volumes/…`）或被 macOS 放在只读临时位置运行时无法自我替换，会提示先拖进「应用程序」。
- 每个版本都用同一张证书签名，更新后辅助功能和麦克风授权保留。

更新清单是 `https://github.com/MoliDuo/MoliWhisper/releases/latest/download/latest.json`，其中的下载地址指向该版本自己的更新包，不用 `latest`。更新包用单独的更新密钥签名（公钥在 `tauri.conf.json`），应用只安装签名有效、且签名里的版本号与清单一致的包。

### 首次使用

1. **授予辅助功能权限**：首次启动时，系统会提示授予辅助功能权限（系统设置 → 隐私与安全性 → 辅助功能），这是监听全局快捷键所必需的。
2. **授予麦克风权限**：首次语音输入时，系统会提示授予麦克风权限。
3. **填写 API Key**：点击菜单栏图标，选择「设置…」。
   - 千问：在[阿里云百炼](https://bailian.console.aliyun.com/)创建 API Key，填入「千问 API Key」，点「保存并测试」。默认模型 `qwen-audio-3.0-asr-flash-streaming`。
   - 自动整理（可选）：勾选后填入 [DeepSeek](https://platform.deepseek.com/) 的 API Key，点「保存并测试」。默认地址 `https://api.deepseek.com`，模型 `deepseek-flash`，使用 OpenAI 兼容的 `/chat/completions` 接口。整理失败或超过 15 秒时粘贴原文。

Key 只保存在本机的配置文件里（`~/Library/Application Support/com.moliduo.moliwhisper/config.json`，权限 0600），不进钥匙串。

### 快捷键

| 操作 | 按键 |
|------|------|
| 开始 / 停止 | 右 `⌥ Option` |
| 取消 | `ESC` |

取消是指放弃本次识别，不复制也不粘贴。

### 使用流程

1. 将光标定位到任意输入框
2. 按下右 `⌥`，屏幕顶部出现悬浮窗，开始说话，悬浮窗实时显示识别到的文字
3. 再次按下右 `⌥` 结束，文字自动粘贴到输入框
4. 想放弃本次识别，按 `ESC`

热键和「切换 / 按住说话」模式可以在设置里改。

## 开发

### 目录

| 路径 | 内容 |
|------|------|
| [`crates/moli-core/`](crates/moli-core) | 纯 Rust 核心：千问（DashScope）语音识别客户端、DeepSeek 文字整理、会话逻辑、音频处理。不依赖 Tauri，可以单独 `cargo test` |
| [`src-tauri/`](src-tauri) | Tauri 应用：托盘、窗口、平台相关代码（热键、粘贴、悬浮窗、权限） |
| [`ui/`](ui) | 悬浮窗和设置页，Vite + 纯 TypeScript |
| [`scripts/`](scripts) | 构建、安装、调试脚本 |
| [`docs/`](docs) | README 用的截图 |

### 环境要求

- Rust stable、Node.js 22+、pnpm
- macOS 13.0+（Xcode Command Line Tools）

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
```

本地构建会自动找钥匙串里的 Apple Development 证书签名，这样重新构建后辅助功能和麦克风授权不会丢。CI 用同一张证书（仓库 Secrets 里的 `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD`）签发布包，所以本地构建和下载的版本共用同一份授权。

更新包另用一把更新密钥签名：`./scripts/setup-updater-keys.sh` 在本机生成一次，公钥写进 `tauri.conf.json`，私钥和密码存进 Secrets 的 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。没有它 CI 不会发布。私钥丢了，已安装的版本就再也收不到自动更新，务必备份。

## License

[MIT](LICENSE)
