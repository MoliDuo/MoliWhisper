import SwiftUI
import AVFoundation

@main
struct DoubaoMurmurApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // Use Settings as a dummy scene; the app is menu-bar only
        Settings {
            EmptyView()
        }
    }
}

@MainActor
class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem!
    private var webViewManager: WebViewManager!
    private var hotkeyManager: HotkeyManager!
    private var transcriptionManager: TranscriptionManager!
    private var overlayPanel: OverlayPanel!
    private let appState = AppState.shared

    func applicationDidFinishLaunching(_ notification: Notification) {
        print("[AppDelegate] applicationDidFinishLaunching")
        setupStatusItem()
        setupOverlay()
        setupWebView()
        setupHotkey()
        setupTranscriptionManager()
        requestMicrophonePermission()
        print("[AppDelegate] ✅ All setup complete")
    }

    private func setupStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = statusItem.button {
            button.image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "Doubao Murmur")
        }
        let menu = NSMenu()
        menu.delegate = self
        statusItem.menu = menu
        rebuildMenu()
    }

    private func rebuildMenu() {
        guard let menu = statusItem.menu else { return }
        menu.removeAllItems()

        let statusTitle: String
        switch appState.loginStatus {
        case .checking:
            statusTitle = "⏳ 检查中..."
        case .loggedIn:
            statusTitle = "✅ 已登录"
        case .notLoggedIn:
            statusTitle = "❌ 未登录"
        }
        let statusMenuItem = NSMenuItem(title: statusTitle, action: nil, keyEquivalent: "")
        statusMenuItem.isEnabled = false
        menu.addItem(statusMenuItem)

        menu.addItem(NSMenuItem.separator())

        if appState.loginStatus != .loggedIn {
            menu.addItem(NSMenuItem(title: "登录豆包", action: #selector(showLogin), keyEquivalent: "l"))
        }

        if appState.loginStatus == .loggedIn {
            menu.addItem(NSMenuItem(title: "退出登录", action: #selector(doLogout), keyEquivalent: ""))
        }

        menu.addItem(NSMenuItem(title: "使用帮助", action: #selector(showHelp), keyEquivalent: "h"))
        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "退出", action: #selector(quitApp), keyEquivalent: "q"))
    }

    @objc private func showLogin() {
        webViewManager.showLoginWindow()
    }

    @objc private func doLogout() {
        webViewManager.logout()
    }

    @objc private func showHelp() {
        let appVersion = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "未知"
        let buildNumber = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "未知"

        let alert = NSAlert()
        alert.messageText = "Doubao Murmur 使用帮助"
        alert.informativeText = """
        版本: \(appVersion) (\(buildNumber))

        语音输入法，基于豆包语音识别。

        快捷键:
        • 按下并释放 右 ⌥ Option — 开始/停止语音识别
        • ESC — 取消当前语音识别

        使用方法:
        1. 首先在菜单中「登录豆包」
        2. 将光标放在任意输入框中
        3. 按 右 ⌥ 开始说话
        4. 再按 右 ⌥ 结束，文本会自动粘贴到输入框

        注意:
        • 需要授予「辅助功能」权限（系统设置 → 隐私与安全性 → 辅助功能）
        • 需要授予「麦克风」权限
        """
        alert.alertStyle = .informational
        alert.addButton(withTitle: "好的")
        alert.runModal()
    }

    @objc private func quitApp() {
        NSApp.terminate(nil)
    }

    private func setupOverlay() {
        overlayPanel = OverlayPanel(appState: appState)
        print("[AppDelegate] Overlay panel created")
    }

    private func setupWebView() {
        webViewManager = WebViewManager(appState: appState)

        // Wire up login status detection from JS interception
        webViewManager.onLoginStatusChange = { [weak self] status, nickname in
            guard let self = self else { return }
            if status == "loggedIn" {
                self.appState.loginStatus = .loggedIn
                print("[AppDelegate] Logged in as: \(nickname ?? "unknown")")
                // Extract ASR params, save to local config, then destroy webview
                self.extractSaveAndDestroyWebView()
            } else {
                self.appState.loginStatus = .notLoggedIn
            }
        }

        // Check if we have saved params — if so, skip loading the webview entirely
        if ASRParamsStore.hasSavedParams {
            appState.loginStatus = .loggedIn
            print("[AppDelegate] ✅ Saved ASR params found, skipping WebView load")
        } else {
            webViewManager.load()
            print("[AppDelegate] No saved params, loading WebView for login")
        }
    }

    /// Extract ASR params from the active webview, save to disk, then destroy the webview.
    private func extractSaveAndDestroyWebView() {
        Task {
            // Wait a moment for cookies/localStorage to fully settle after login
            try? await Task.sleep(nanoseconds: 1_000_000_000) // 1 second

            if let params = await webViewManager.extractASRParams() {
                ASRParamsStore.save(params)
                print("[AppDelegate] ✅ ASR params saved, destroying WebView")
            } else {
                print("[AppDelegate] ⚠️ Failed to extract ASR params after login")
            }

            // Destroy webview to free resources regardless
            webViewManager.hideLoginWindow()
            webViewManager.destroyWebView()
        }
    }

    /// Handle auth expiration: prompt user and re-open webview if they agree.
    private func handleAuthExpired() {
        let alert = NSAlert()
        alert.messageText = "认证已过期"
        alert.informativeText = "豆包登录凭证已失效，需要重新登录以继续使用语音识别。"
        alert.addButton(withTitle: "重新登录")
        alert.addButton(withTitle: "取消")
        alert.alertStyle = .warning

        NSApp.activate(ignoringOtherApps: true)
        let response = alert.runModal()
        if response == .alertFirstButtonReturn {
            webViewManager.showLoginWindow()
        }
    }

    private func setupHotkey() {
        hotkeyManager = HotkeyManager()
        print("[AppDelegate] Hotkey manager created")
    }

    private func requestMicrophonePermission() {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            print("[AppDelegate] 🎤 Microphone permission already granted")
        case .notDetermined:
            print("[AppDelegate] 🎤 Requesting microphone permission...")
            AVCaptureDevice.requestAccess(for: .audio) { granted in
                print("[AppDelegate] 🎤 Microphone permission \(granted ? "granted" : "denied")")
            }
        case .denied, .restricted:
            print("[AppDelegate] ⚠️ Microphone permission denied/restricted. Go to System Settings → Privacy & Security → Microphone")
        @unknown default:
            break
        }
    }

    private func setupTranscriptionManager() {
        transcriptionManager = TranscriptionManager(
            appState: appState,
            webViewManager: webViewManager,
            overlayPanel: overlayPanel,
            hotkeyManager: hotkeyManager
        )

        // Handle auth expiration — prompt user to re-login
        transcriptionManager.onAuthExpired = { [weak self] in
            self?.handleAuthExpired()
        }

        transcriptionManager.start()
        print("[AppDelegate] Transcription manager started")
    }
}

extension AppDelegate: NSMenuDelegate {
    func menuWillOpen(_ menu: NSMenu) {
        rebuildMenu()
    }
}
