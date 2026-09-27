import AppKit
import SwiftUI

class OverlayPanel: NSPanel {
    private let appState: AppState
    var onCancel: (() -> Void)?

    override var canBecomeKey: Bool { true }

    init(appState: AppState) {
        self.appState = appState

        let width: CGFloat = 420
        let height: CGFloat = 60

        // Position at top-center of main screen
        let screenFrame = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let x = screenFrame.midX - width / 2
        let y = screenFrame.maxY - height - 20

        super.init(
            contentRect: NSRect(x: x, y: y, width: width, height: height),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        self.level = .floating
        self.isOpaque = false
        self.backgroundColor = .clear
        self.hasShadow = true
        self.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        self.isMovableByWindowBackground = true
        self.hidesOnDeactivate = false

        let hostingView = NSHostingView(rootView: OverlayView(appState: appState))
        self.contentView = hostingView
    }

    func showOverlay() {
        print("[OverlayPanel] showOverlay called")
        // Reposition to top-center in case screen changed
        if let screen = NSScreen.main {
            let width: CGFloat = 420
            let height: CGFloat = 60
            let x = screen.visibleFrame.midX - width / 2
            let y = screen.visibleFrame.maxY - height - 20
            let frame = NSRect(x: x, y: y, width: width, height: height)
            print("[OverlayPanel] Setting frame: \(frame)")
            setFrame(frame, display: true)
        }
        orderFrontRegardless()
        makeKey()
        print("[OverlayPanel] ✅ Overlay is now visible (isVisible=\(isVisible), isKeyWindow=\(isKeyWindow))")
    }

    func hideOverlay() {
        print("[OverlayPanel] hideOverlay called")
        resignKey()
        orderOut(nil)
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { // ESC
            print("[OverlayPanel] ESC key received via keyDown")
            onCancel?()
        } else {
            super.keyDown(with: event)
        }
    }
}
