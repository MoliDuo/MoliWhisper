import Foundation
import AppKit

struct PasteHelper {
    static func copyAndPaste(_ text: String) {
        guard !text.isEmpty else { return }

        // Copy to clipboard
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)

        // Short delay then simulate ⌘V
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
            simulatePaste()
        }
    }

    static func copyOnly(_ text: String) {
        guard !text.isEmpty else { return }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
    }

    private static func simulatePaste() {
        let source = CGEventSource(stateID: .hidSystemState)

        // Key down: ⌘V
        let keyDown = CGEvent(keyboardEventSource: source, virtualKey: 0x09, keyDown: true) // 0x09 = V
        keyDown?.flags = .maskCommand

        // Key up: ⌘V
        let keyUp = CGEvent(keyboardEventSource: source, virtualKey: 0x09, keyDown: false)
        keyUp?.flags = .maskCommand

        keyDown?.post(tap: .cghidEventTap)
        keyUp?.post(tap: .cghidEventTap)
    }
}
