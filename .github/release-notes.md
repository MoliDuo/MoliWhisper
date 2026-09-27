## 下载

- **macOS**（Apple Silicon，13.0+）：`MoliWhisper_*_aarch64.dmg`
- **Windows**（x64）：`MoliWhisper_*_x64-setup.exe`，安装到当前用户，不需要管理员权限

## macOS 首次打开

安装包只做了 ad-hoc 签名，没有公证。拖进「应用程序」后，第一次打开会被 Gatekeeper 拦住，可以：

- 在「系统设置 → 隐私与安全性」里点「仍要打开」；或者
- 在终端执行 `xattr -dr com.apple.quarantine /Applications/MoliWhisper.app`

然后按提示授予**辅助功能**（全局热键和自动粘贴）和**麦克风**权限。ad-hoc 签名每个版本都不同，升级后可能需要在辅助功能列表里删掉旧条目、重新授权。

## Windows

目前只有登录和托盘；全局热键和自动粘贴还在开发中。
