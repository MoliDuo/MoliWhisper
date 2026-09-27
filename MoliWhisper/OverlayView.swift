import SwiftUI

struct OverlayView: View {
    @ObservedObject var appState: AppState

    var body: some View {
        HStack(spacing: 12) {
            // State indicator: spinner when starting, pulsing dot when recording
            if appState.recordingState == .starting {
                SpinnerView()
                    .frame(width: 14, height: 14)
            } else {
                Circle()
                    .fill(Color.red)
                    .frame(width: 10, height: 10)
                    .opacity(appState.recordingState == .recording ? 1.0 : 0.5)
                    .animation(
                        appState.recordingState == .recording
                            ? .easeInOut(duration: 0.6).repeatForever(autoreverses: true)
                            : .default,
                        value: appState.recordingState == .recording
                    )
            }

            if let error = appState.errorMessage {
                Text(error)
                    .foregroundColor(.red)
                    .font(.system(size: 14, weight: .medium))
                    .lineLimit(1)
            } else if appState.transcriptionText.isEmpty {
                Text(statusText)
                    .foregroundColor(.gray)
                    .font(.system(size: 14))
                    .lineLimit(1)
            } else {
                Text(appState.transcriptionText)
                    .foregroundColor(.white)
                    .font(.system(size: 14, weight: .medium))
                    .lineLimit(2)
                    .truncationMode(.head)
            }

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color(white: 0.2).opacity(0.95))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color.white.opacity(0.25), lineWidth: 1)
        )
    }

    private var statusText: String {
        switch appState.recordingState {
        case .idle:
            return ""
        case .starting:
            return "正在启动语音识别..."
        case .recording:
            return "正在聆听..."
        case .stopping:
            return "正在处理..."
        }
    }
}

// MARK: - Spinner for loading state

private struct SpinnerView: View {
    @State private var isAnimating = false

    var body: some View {
        Circle()
            .trim(from: 0.1, to: 0.9)
            .stroke(Color.white.opacity(0.8), style: StrokeStyle(lineWidth: 2, lineCap: .round))
            .rotationEffect(.degrees(isAnimating ? 360 : 0))
            .animation(
                .linear(duration: 0.8).repeatForever(autoreverses: false),
                value: isAnimating
            )
            .onAppear { isAnimating = true }
    }
}
