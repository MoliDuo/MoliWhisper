# Plan: MoliWhisper macOS Voice Input App

## TL;DR
Build a Swift + SwiftUI menu bar app that embeds a hidden WKWebView loading doubao.com/chat, intercepts its ASR (speech recognition) WebSocket messages via JS injection, and presents real-time transcription in a floating overlay. Right ⌥ Option toggles recording; ESC cancels. Final text is copied to clipboard and auto-pasted.

## Architecture Overview

```
┌─────────────────────────────────────────────┐
│  MoliWhisper (menu bar app, no Dock icon) │
├─────────────────────────────────────────────┤
│  HotkeyManager                              │
│  ├─ CGEvent tap for Right ⌥ / ESC           │
│  └─ Requires Accessibility permission       │
├─────────────────────────────────────────────┤
│  WebViewManager (hidden WKWebView)          │
│  ├─ Loads https://www.doubao.com/chat       │
│  ├─ JS injection: WebSocket monkey-patch    │
│  ├─ JS injection: DOM interaction (asr_btn) │
│  ├─ WKContentRuleList: block /completion    │
│  ├─ WKScriptMessageHandler: receive ASR     │
│  └─ WKUIDelegate: auto-grant mic permission │
├─────────────────────────────────────────────┤
│  OverlayPanel (NSPanel, floating)           │
│  ├─ Top-center of screen                    │
│  ├─ Shows real-time transcription text      │
│  └─ Semi-transparent, non-activating        │
├─────────────────────────────────────────────┤
│  TranscriptionManager (orchestrator)        │
│  ├─ State machine: idle → recording → done  │
│  └─ Coordinates all components              │
├─────────────────────────────────────────────┤
│  PasteHelper                                │
│  ├─ NSPasteboard: copy text                 │
│  └─ CGEvent: simulate ⌘V paste             │
└─────────────────────────────────────────────┘
```

## Steps

### Phase 1: Project Scaffold & Menu Bar App
1. Create Xcode project `MoliWhisper` (macOS, SwiftUI App lifecycle)
2. Configure as menu bar-only app: `LSUIElement = true` in Info.plist (hide Dock icon)
3. Create `AppDelegate` with `NSStatusItem` for menu bar icon (microphone icon SF Symbol `mic.fill`)
4. Menu bar dropdown:
   - Status indicator (⏳ 检查中 / ✅ 已登录 / ❌ 未登录)
   - "登录豆包" — 显示 WebView 登录窗口（未登录 / 检查中 时显示）
   - "退出登录" — 清除 doubao cookie 并重新加载（已登录时显示）
   - "重新加载" — 重新加载 WebView
   - "使用帮助" — 弹出使用说明对话框
   - "退出" — 退出应用

### Phase 2: Hidden WKWebView + Login Flow
5. Create `WebViewManager` — initializes a `WKWebView` in a hidden `NSWindow`
6. Configure `WKWebViewConfiguration`:
   - `WKUserContentController` for JS injection and message handlers
   - `WKContentRuleList` to block `doubao.com/chat/completion` requests
   - `WKPreferences` with `javaScriptEnabled = true`
7. Load `https://www.doubao.com/chat` on app launch
8. **Login detection** — 通过 JS 注入在 document start 阶段拦截网络请求:
   - Monkey-patch `window.fetch` 和 `XMLHttpRequest`，拦截 `/alice/profile/self` API 调用
   - Profile API 返回 `code: 0` 且包含 `profile_brief` → 发送 `login:loggedIn` 消息给 Swift
   - Profile API 返回错误 → 发送 `login:notLoggedIn` 消息给 Swift
   - Fallback: 延迟检查 DOM 中 `button[data-testid="to_login_button"]` 是否存在
   - 默认状态为 `.checking`，只有收到明确信号后才切换状态
9. If not logged in → 菜单显示"登录豆包"选项，用户点击后显示 WebView 登录窗口
10. After login (profile API 拦截到成功响应) → 自动隐藏 WebView 窗口，更新菜单栏状态
11. **Logout** — 清除 WKWebsiteDataStore 中 doubao 相关的所有数据（cookies, localStorage 等），然后重新加载页面

### Phase 3: JS Injection — WebSocket Interception
11. Create `inject.js` as a `WKUserScript` injected at **document start** (before page scripts run):
    - Monkey-patch `WebSocket` constructor to intercept connections to `samantha/audio/asr`
    - On ASR WebSocket `message` event → parse JSON → post `{event, text}` to `window.webkit.messageHandlers.asrHandler.postMessage()`
    - On ASR WebSocket `close` / `error` → notify Swift
12. Create a second JS helper injected at **document end** for DOM interactions:
    - `clickAsrButton()` — finds `[data-testid="asr_btn"]` and clicks it
    - `getAsrButtonState()` — returns current `data-state` value
    - `isLoginButtonPresent()` — checks for login button
13. Register `WKScriptMessageHandler` in Swift to receive messages from JS

### Phase 4: Floating Overlay UI
14. Create `OverlayPanel` subclass of `NSPanel`:
    - Style: `.nonactivatingPanel`, `.borderless`, background `NSColor.black.withAlphaComponent(0.75)`
    - Window level: `.floating` (above normal windows)
    - Size: ~400×80, positioned top-center of main screen
    - Content: SwiftUI view with recording indicator (pulsing dot) + transcription `Text`
15. Overlay shows on recording start, hides on stop/cancel
16. Text updates in real-time as WSS messages arrive

### Phase 5: Global Hotkey (Right ⌥ Option + ESC)
17. Create `HotkeyManager` using `CGEvent.tapCreate()` with `.cgSessionEventTap`:
    - Listen for `.flagsChanged` events
    - Detect Right Option via `CGEventFlags.maskAlternate` + raw flag `0x00000040` (NX_DEVICERALTKEYMASK)
    - Detect ESC via `.keyDown` with keyCode 53
18. **Requires Accessibility permission** — prompt user on first launch via `AXIsProcessTrustedWithOptions`
19. Right Option press-and-release (no other key pressed) → toggle recording
20. ESC → cancel current recording

### Phase 6: Transcription Orchestrator
21. Create `TranscriptionManager` with state machine:
    - **States**: `idle` → `starting` → `recording` → `stopping` → `idle`
    - `idle → starting`: Right ⌥ pressed → call JS `clickAsrButton()` → show overlay
    - `starting → recording`: ASR button state becomes `active` + first WSS message received
    - `recording → stopping`: Right ⌥ pressed again → call JS `clickAsrButton()` to stop
    - `stopping → idle`: WSS `finish` event received → copy text → paste → hide overlay
    - Any state → `idle`: ESC pressed → if ASR active, click button to deactivate → hide overlay, discard text
22. Handle edge cases:
    - Double-press protection (debounce Right ⌥, ~300ms)
    - WSS connection failure → show error in overlay, auto-dismiss after 3s
    - Empty transcription → don't paste

### Phase 7: Clipboard & Auto-Paste
23. Create `PasteHelper`:
    - Copy final transcription text to `NSPasteboard.general`
    - Simulate ⌘V using `CGEvent(keyboardEventSource:virtualKey:keyDown:)` with key code 9 (V) + `.maskCommand`
    - Small delay (~50ms) between clipboard write and paste simulation
24. Only auto-paste if a text input is focused (best-effort; paste simulation will be a no-op if no input is focused)

### Phase 8: Microphone Permission Handling
25. Implement `WKUIDelegate.webView(_:requestMediaCapturePermissionFor:initiatedByFrame:type:decisionHandler:)`:
    - Auto-grant `.microphone` permission (`decisionHandler(.grant)`)
26. App's `Info.plist` must include `NSMicrophoneUsageDescription`

## Project Structure

```
MoliWhisper/
├── MoliWhisper.xcodeproj
└── MoliWhisper/
    ├── MoliWhisperApp.swift          — @main, SwiftUI App with menu bar scene
    ├── AppState.swift                 — ObservableObject shared state
    ├── MenuBarView.swift              — Menu bar dropdown UI
    ├── HotkeyManager.swift            — CGEvent tap for Right ⌥ / ESC
    ├── WebViewManager.swift           — WKWebView lifecycle + JS bridge
    ├── OverlayPanel.swift             — Floating NSPanel + SwiftUI content
    ├── OverlayView.swift              — SwiftUI view for overlay content
    ├── TranscriptionManager.swift     — State machine orchestrator
    ├── PasteHelper.swift              — Clipboard + ⌘V simulation
    ├── Resources/
    │   ├── inject-websocket.js        — WebSocket monkey-patch (document start)
    │   ├── inject-dom.js              — DOM helper functions (document end)
    │   └── Assets.xcassets
    ├── Info.plist
    └── MoliWhisper.entitlements     — App Sandbox + network + mic
```

## Relevant Files (to create)

- `Info.plist` — `LSUIElement=true`, `NSMicrophoneUsageDescription`, App Transport Security exceptions
- `MoliWhisper.entitlements` — `com.apple.security.app-sandbox`, `com.apple.security.network.client`, `com.apple.security.device.microphone`, `com.apple.security.device.audio-input`
- `inject-websocket.js` — Core JS: monkey-patches `WebSocket` (ASR 拦截) + `fetch`/`XMLHttpRequest` (profile API 登录检测), forwards to Swift via `webkit.messageHandlers`
- `inject-dom.js` — DOM interaction: `clickAsrButton()`, `getAsrButtonState()`, `isLoginButtonPresent()`
- `WebViewManager.swift` — Heaviest component: WKWebView config, content rule list for blocking `/chat/completion`, JS injection, WKScriptMessageHandler, WKUIDelegate for mic, WKNavigationDelegate for login redirect detection

## Verification

1. **Login flow**: Launch app → menu bar shows "未登录" → click "Show Login" → WebView window appears → complete login → window auto-hides → menu bar shows "已登录"
2. **Recording toggle**: Press Right ⌥ → overlay appears with pulsing indicator → speak → real-time text appears in overlay → press Right ⌥ again → overlay disappears → check clipboard contains transcription
3. **ESC cancel**: Start recording → press ESC → overlay disappears → clipboard unchanged
4. **Completion blocking**: During recording, verify in Console.app/Xcode logs that `/chat/completion` requests are blocked
5. **Auto-paste**: Focus a TextEdit window → do a full recording cycle → verify text is pasted into TextEdit
6. **Edge cases**: Test double-press, no mic permission, network failure, empty transcription

## Decisions

- **Right ⌥ detection**: Use CGEvent tap (requires Accessibility permission) — this is the only reliable way to distinguish left/right modifier keys on macOS
- **WebSocket interception via JS injection**: Monkey-patching WebSocket at document start is the cleanest approach; avoids needing a custom URL protocol handler or MITM proxy
- **Block /chat/completion via WKContentRuleList**: Native WebKit content blocker — no JS needed, reliable
- **Auto-grant mic to WKWebView**: Since this is a self-use voice input app, auto-granting is appropriate; the OS will still show its own mic permission prompt for the app on first use
- **No App Sandbox relaxation needed for CGEvent tap**: Actually, CGEvent tap requires the app to NOT be sandboxed, OR have Accessibility permission. For self-use, we can either disable sandbox or ensure Accessibility is granted. **Recommendation: disable App Sandbox** for simplicity since this is self-use only. CGEvent taps and accessibility APIs don't work well in sandbox.

## Further Considerations

1. **WKWebView cookie persistence**: WKWebView uses `WKWebsiteDataStore.default()` which persists cookies across app launches. User only needs to log in once. Verify this works correctly — if not, may need `HTTPCookieStorage` bridging.
2. **Page reload resilience**: If doubao.com updates their page structure (button test IDs, WSS endpoint), the app will break. Consider adding a "health check" that verifies expected DOM elements exist on page load and alerts the user if something is wrong.
3. **Right ⌥ as toggle vs hold-to-talk**: Current design is toggle (press to start, press to stop). An alternative is push-to-talk (hold to record, release to stop). Toggle is chosen per the spec, but hold-to-talk could be a future option.
