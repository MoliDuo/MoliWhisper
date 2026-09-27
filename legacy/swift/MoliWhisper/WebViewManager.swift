import Foundation
import WebKit
import Combine

/// NSWindow subclass that always reports its occlusion state as visible.
/// This prevents WebKit from throttling JS timers, suspending media capture,
/// or marking the page as hidden via the Page Visibility API.
private class AlwaysActiveWindow: NSWindow {
    override var occlusionState: OcclusionState { .visible }
}

@MainActor
class WebViewManager: NSObject {
    private let appState: AppState
    private var webView: WKWebView?
    private var webViewWindow: NSWindow?

    /// Whether the WKWebView is currently loaded and active.
    var isActive: Bool { webView != nil }

    // Callback for login status changes
    var onLoginStatusChange: ((_ status: String, _ nickname: String?) -> Void)?

    init(appState: AppState) {
        self.appState = appState
        super.init()
        // WebView is NOT created here; call load() to create on demand.
    }

    private func setupWebView() {
        guard webView == nil else { return }

        let config = WKWebViewConfiguration()

        // User content controller for JS injection + message handling
        let userContent = WKUserContentController()

        // Inject login detector + visibility override at document start
        if let wsJS = loadJSResource("inject-websocket") {
            let wsScript = WKUserScript(source: wsJS, injectionTime: .atDocumentStart, forMainFrameOnly: false)
            userContent.addUserScript(wsScript)
        }

        // Inject DOM helpers at document end
        if let domJS = loadJSResource("inject-dom") {
            let domScript = WKUserScript(source: domJS, injectionTime: .atDocumentEnd, forMainFrameOnly: true)
            userContent.addUserScript(domScript)
        }

        // Register message handler
        userContent.add(self, name: "asrHandler")

        config.userContentController = userContent
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let wv = WKWebView(frame: NSRect(x: 0, y: 0, width: 1280, height: 800), configuration: config)
        wv.navigationDelegate = self
        wv.uiDelegate = self
        wv.customUserAgent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36"
        self.webView = wv

        // Create window to host webview (AlwaysActiveWindow keeps WebKit alive)
        let window = AlwaysActiveWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1280, height: 800),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "MoliWhisper - Login"
        window.contentView = wv
        window.isReleasedWhenClosed = false
        self.webViewWindow = window
        enterBackgroundMode()
    }

    /// Keep the window "visible" to the system so WebKit won't suspend
    /// JS execution, but hidden from the user by placing it below the
    /// desktop level and excluding it from Mission Control / Cmd+Tab.
    private func enterBackgroundMode() {
        guard let window = webViewWindow else { return }
        window.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopWindow)) - 1)
        window.collectionBehavior = [.transient, .ignoresCycle]
        window.ignoresMouseEvents = true
        if !window.isVisible {
            window.orderBack(nil)
        }
    }

    private func loadJSResource(_ name: String) -> String? {
        guard let url = Bundle.main.url(forResource: name, withExtension: "js") else {
            print("[WebViewManager] JS resource not found: \(name)")
            return nil
        }
        return try? String(contentsOf: url, encoding: .utf8)
    }

    // MARK: - Public API

    /// Create the webview (if needed) and load doubao.com.
    func load() {
        if webView == nil {
            setupWebView()
        }
        let url = URL(string: "https://www.doubao.com/chat")!
        webView?.load(URLRequest(url: url))
    }

    func reload() {
        appState.loginStatus = .checking
        if let wv = webView {
            wv.reload()
        } else {
            load()
        }
    }

    func showLoginWindow() {
        // Ensure webview exists before showing
        if webView == nil {
            load()
        }
        guard let window = webViewWindow else { return }
        window.level = .normal
        window.collectionBehavior = []
        window.ignoresMouseEvents = false
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func hideLoginWindow() {
        enterBackgroundMode()
    }

    /// Completely destroy the WKWebView and its hosting window to free resources.
    func destroyWebView() {
        webView?.stopLoading()
        webView?.configuration.userContentController.removeScriptMessageHandler(forName: "asrHandler")
        webView?.configuration.userContentController.removeAllUserScripts()
        webView?.navigationDelegate = nil
        webView?.uiDelegate = nil
        webView = nil
        webViewWindow?.orderOut(nil)
        webViewWindow?.contentView = nil
        webViewWindow = nil
        print("[WebViewManager] ♻️ WebView destroyed to free resources")
    }

    func checkLoginState() {
        guard let wv = webView else { return }
        // Login detection is handled by fetch/XHR interception in inject-websocket.js.
        // As a fallback, check the DOM after a delay.
        wv.evaluateJavaScript("window.__moliWhisper.isLoginButtonPresent()") { [weak self] result, error in
            guard let self = self else { return }
            Task { @MainActor in
                if let isLoginButton = result as? Bool, isLoginButton {
                    if self.appState.loginStatus == .checking {
                        self.appState.loginStatus = .notLoggedIn
                    }
                }
            }
        }
    }

    func logout() {
        // Clear saved params
        ASRParamsStore.clear()
        // Clear WKWebsiteDataStore (works even without an active webview)
        let dataStore = WKWebsiteDataStore.default()
        let allTypes = WKWebsiteDataStore.allWebsiteDataTypes()
        dataStore.fetchDataRecords(ofTypes: allTypes) { records in
            let doubaoRecords = records.filter { $0.displayName.contains("doubao") }
            dataStore.removeData(ofTypes: allTypes, for: doubaoRecords) { [weak self] in
                Task { @MainActor in
                    self?.appState.loginStatus = .notLoggedIn
                    // If webview is active, reload it to show login page
                    if self?.webView != nil {
                        self?.load()
                    }
                }
            }
        }
    }

    // MARK: - ASR Parameter Extraction

    /// Extract cookies and localStorage parameters needed for native WSS ASR connection.
    func extractASRParams() async -> DoubaoASRParams? {
        guard let wv = webView else {
            print("[WebViewManager] ⚠️ WebView not active, cannot extract params")
            return nil
        }

        // 1. Extract all doubao.com cookies (including httpOnly session cookies)
        let cookies = await withCheckedContinuation { continuation in
            wv.configuration.websiteDataStore.httpCookieStore.getAllCookies { cookies in
                continuation.resume(returning: cookies)
            }
        }
        let doubaoCookies = cookies.filter { $0.domain.contains("doubao.com") }

        guard !doubaoCookies.isEmpty else {
            print("[WebViewManager] ⚠️ No doubao cookies found")
            return nil
        }

        // 2. Extract device_id from localStorage
        var deviceId = ""
        if let raw = try? await wv.evaluateJavaScript(
            "localStorage.getItem('samantha_web_web_id')"
        ) as? String,
           let data = raw.data(using: .utf8),
           let parsed = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            deviceId = (parsed["web_id"] as? String) ?? ""
        }

        // 3. Extract web_id / tea_uuid from localStorage
        var webId = ""
        if let raw = try? await wv.evaluateJavaScript(
            "localStorage.getItem('__tea_cache_tokens_497858')"
        ) as? String,
           let data = raw.data(using: .utf8),
           let parsed = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            webId = (parsed["web_id"] as? String) ?? ""
        }

        guard !deviceId.isEmpty, !webId.isEmpty else {
            print("[WebViewManager] ⚠️ Failed to extract localStorage params (deviceId=\(deviceId), webId=\(webId))")
            return nil
        }

        let params = DoubaoASRParams(httpCookies: doubaoCookies, deviceId: deviceId, webId: webId)
        print("[WebViewManager] ✅ ASR params extracted (cookies=\(doubaoCookies.count), deviceId=\(deviceId), webId=\(webId))")
        return params
    }
}

// MARK: - WKScriptMessageHandler
extension WebViewManager: WKScriptMessageHandler {
    nonisolated func userContentController(
        _ userContentController: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        guard message.name == "asrHandler",
              let body = message.body as? [String: Any],
              let type = body["type"] as? String else { return }

        if type == "login" {
            let status = body["status"] as? String ?? "unknown"
            let nickname = body["nickname"] as? String
            Task { @MainActor in
                self.onLoginStatusChange?(status, nickname)
            }
            return
        }

        if type == "debug" {
            let text = body["text"] as? String ?? ""
            print("[WebViewManager] [JS Debug] \(text)")
            return
        }
    }
}

// MARK: - WKNavigationDelegate
extension WebViewManager: WKNavigationDelegate {
    nonisolated func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        Task { @MainActor in
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.checkLoginState()
            }
        }
    }

    nonisolated func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        if let url = navigationAction.request.url,
           url.absoluteString.contains("from_login=1") {
            Task { @MainActor in
                self.appState.loginStatus = .loggedIn
                self.hideLoginWindow()
            }
        }
        decisionHandler(.allow)
    }
}

// MARK: - WKUIDelegate
extension WebViewManager: WKUIDelegate {
    // Auto-grant microphone permission
    nonisolated func webView(
        _ webView: WKWebView,
        requestMediaCapturePermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo,
        type: WKMediaCaptureType,
        decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        decisionHandler(.grant)
    }

    // Handle window.open (e.g., login popups)
    nonisolated func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        if navigationAction.targetFrame == nil {
            webView.load(navigationAction.request)
        }
        return nil
    }
}
