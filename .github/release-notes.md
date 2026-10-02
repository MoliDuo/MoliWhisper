## 下载

- **macOS**（Apple Silicon，13.0+）：`MoliWhisper_*_aarch64.dmg`

已经装了的不用下载：应用会自己检查并提示更新，也可以在菜单栏点「检查更新…」。`MoliWhisper.app.tar.gz` 和 `latest.json` 是给自动更新用的。

## macOS 首次打开

安装包用 Apple Development 证书签名，但没有公证。从浏览器下载的，拖进「应用程序」后第一次打开会被 Gatekeeper 拦住，可以：

- 在「系统设置 → 隐私与安全性」里点「仍要打开」；或者
- 在终端执行 `xattr -dr com.apple.quarantine /Applications/MoliWhisper.app`

用仓库里的 `scripts/update.sh` 安装（经 `gh` 下载）不会被拦。

然后按提示授予**辅助功能**（全局热键和自动粘贴）和**麦克风**权限。每个版本的签名相同，升级后授权保留。
