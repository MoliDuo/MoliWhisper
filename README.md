# Moli Whisper

macOS 上的全局语音输入：按下右 `⌥ Option` 开始说话，再按一次结束，识别结果自动粘贴到当前光标所在的输入框。

[![ci](https://github.com/MoliDuo/MoliWhisper/actions/workflows/ci.yml/badge.svg)](https://github.com/MoliDuo/MoliWhisper/actions/workflows/ci.yml)
[![最新版本](https://img.shields.io/github/v/release/MoliDuo/MoliWhisper)](https://github.com/MoliDuo/MoliWhisper/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

fork 自 [lilong7676/doubao-murmur](https://github.com/lilong7676/doubao-murmur)（MIT），语音识别已换成阿里云百炼的千问实时语音识别，另加了用 DeepSeek 把口语整理成书面语的「自动整理」。

<p align="center">
  <img src="docs/screenshots/overlay_pannel.png" width="500" alt="语音识别悬浮窗">
</p>

## 功能

- 全局热键说话，悬浮窗实时显示识别到的文字，结束后自动粘贴到当前输入框。
- 可选的自动整理：DeepSeek 把口语改成书面语，整理失败或超过 15 秒时粘贴原文。
- 「切换」或「按住说话」两种模式，热键可以在设置里改。
- 应用内自动更新，更新后辅助功能和麦克风授权保留。

### 快捷键

| 操作        | 按键            |
| ----------- | --------------- |
| 开始 / 停止 | 右 `⌥ Option`   |
| 取消        | `ESC`           |

取消是指放弃本次识别，不复制也不粘贴。

### 使用流程

1. 将光标定位到任意输入框
2. 按下右 `⌥`，屏幕顶部出现悬浮窗，开始说话，悬浮窗实时显示识别到的文字
3. 再次按下右 `⌥` 结束，文字自动粘贴到输入框
4. 想放弃本次识别，按 `ESC`

## 安装

要求 macOS 13.0+（Apple 芯片）。从 [Releases](https://github.com/MoliDuo/MoliWhisper/releases/latest) 下载 `MoliWhisper_X.Y.Z_macos_arm64.dmg`，打开后把 MoliWhisper 拖进「应用程序」。

安装包用自签名证书签名，没有经过 Apple 公证：第一次打开被拦住时，到「系统设置 → 隐私与安全性」点「仍要打开」。也可以用 [gh](https://cli.github.com) 安装，这样不会被拦：

```bash
./scripts/update.sh
```

### 首次使用

1. **授予辅助功能权限**：首次启动时，系统会提示授予辅助功能权限（系统设置 → 隐私与安全性 → 辅助功能），这是监听全局快捷键和自动粘贴所必需的。
2. **授予麦克风权限**：首次语音输入时，系统会提示授予麦克风权限。
3. **填写 API Key**：点击菜单栏图标，选择「设置…」。
   - 千问：在[阿里云百炼](https://bailian.console.aliyun.com/)创建 API Key，填入「千问 API Key」，点「保存并测试」。默认模型 `qwen-audio-3.0-asr-flash-streaming`。
   - 自动整理（可选）：勾选后填入 [DeepSeek](https://platform.deepseek.com/) 的 API Key，点「保存并测试」。默认地址 `https://api.deepseek.com`，模型 `deepseek-flash`，使用 OpenAI 兼容的 `/chat/completions` 接口。

### 自动更新

- 启动约 10 秒后和之后每小时检查一次新版本，没有新版本或连不上时不打扰。
- 发现新版本时弹窗询问，选「立即更新」才下载、替换并重启；选「稍后」则本次运行不再提醒这个版本。
- 菜单栏的「检查更新…」和设置页「通用」里的按钮可以手动检查，会明确告诉你「已是最新版本」或失败原因。
- 从 dmg 里直接运行或被 macOS 放在只读临时位置运行时无法更新，会提示先把应用移到「应用程序」文件夹。

更新清单是 `https://github.com/MoliDuo/MoliWhisper/releases/latest/download/latest.json`，其中的下载地址指向该版本自己的更新包。更新包用单独的更新密钥签名（公钥在 `src-tauri/tauri.conf.json`），应用只安装签名有效的包。

## 登录方式

不需要登录，也没有账号。语音识别和自动整理用你自己填的 API Key，只保存在本机的配置文件里（`~/Library/Application Support/com.moliduo.moliwhisper/config.json`，权限 0600），不进钥匙串，也不上传到 Moli 的服务器。

## 部署

无。这是桌面应用，通过 [GitHub Releases](https://github.com/MoliDuo/MoliWhisper/releases) 分发：在 `main` 上给 `chore(release): vX.Y.Z` 这个提交打标签 `vX.Y.Z` 并推送，`.github/workflows/release.yml` 会构建、签名、发布并核对线上的更新清单。步骤见 [AGENTS.md](AGENTS.md)。

## 开发

### 目录

| 路径                                    | 内容                                                                                                                  |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| [`crates/moli-core/`](crates/moli-core) | 纯 Rust 核心：千问（DashScope）语音识别客户端、DeepSeek 文字整理、会话逻辑、音频处理。不依赖 Tauri，可以单独 `cargo test` |
| [`src-tauri/`](src-tauri)               | Tauri 应用：托盘、窗口、更新、平台相关代码（热键、粘贴、悬浮窗、权限）                                                 |
| [`ui/`](ui)                             | 悬浮窗和设置页，Vite + 纯 TypeScript                                                                                    |
| [`scripts/`](scripts)                   | 构建、安装、调试脚本；`scripts/release/` 是发版流水线用的更新清单、签名校验和发布说明                                     |
| [`docs/`](docs)                         | 架构说明和截图                                                                                                          |

### 环境要求

- macOS 13.0+（Xcode Command Line Tools）
- Rust（版本固定在 `rust-toolchain.toml`）、Node.js 24（`.nvmrc`）、pnpm

### 常用命令

```bash
# 和 CI 一样的检查：格式、类型、测试、构建、rustfmt、clippy、cargo test
pnpm install --frozen-lockfile
pnpm run check

# 构建带签名的 debug .app 并启动（测试热键、粘贴、麦克风用这个）
./scripts/dev-app.sh

# 只调界面：tauri dev（权限会记在终端名下）
./scripts/dev.sh

# 构建 release 版，安装到 /Applications 并启动
./scripts/install.sh

# 从 GitHub 装最新发布的版本（或指定标签）
./scripts/update.sh [v2.1.0]

# 改版本号（只改 package.json，发版用）
./scripts/set-version.sh 2.1.0

# 看日志 / 结束进程 / 重置系统授权
./scripts/logs.sh
./scripts/kill.sh
./scripts/reset-tcc.sh
```

本地构建会自动找钥匙串里的「Moli Self-Signed Code Signing」自签名证书签名（Moli 系列 macOS 应用共用），这样重新构建后辅助功能和麦克风授权不会丢。发布包由 CI 用同一张证书签名（组织密钥 `CODESIGN_P12_BASE64` / `CODESIGN_P12_PASSWORD`）。

更新包另用一把更新密钥签名：公钥在 `tauri.conf.json`，私钥和密码在 Actions 密钥 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。私钥丢了，已安装的版本就再也收不到自动更新。

## 许可

[MIT](LICENSE)。上游 [lilong7676/doubao-murmur](https://github.com/lilong7676/doubao-murmur) 的版权声明保留在 LICENSE 里。
