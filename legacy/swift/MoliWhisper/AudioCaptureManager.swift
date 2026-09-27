import Foundation
import AVFoundation

/// Captures microphone audio using AVAudioEngine, resamples to 16kHz mono,
/// and delivers Int16 little-endian PCM data via the `onAudioData` callback.
class AudioCaptureManager {
    private var audioEngine: AVAudioEngine?
    private var converter: AVAudioConverter?
    private var isCapturing = false

    /// Called on the audio thread with raw Int16 LE PCM data ready to send over WebSocket.
    var onAudioData: ((Data) -> Void)?

    func startCapture() throws {
        guard !isCapturing else { return }

        let engine = AVAudioEngine()
        let inputNode = engine.inputNode
        let inputFormat = inputNode.inputFormat(forBus: 0)

        guard inputFormat.sampleRate > 0, inputFormat.channelCount > 0 else {
            throw NSError(domain: "AudioCapture", code: -1,
                          userInfo: [NSLocalizedDescriptionKey: "No audio input available"])
        }

        // Target: 16kHz mono Float32 (converted to Int16 manually per doubao spec)
        let targetFormat = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: 16000,
            channels: 1,
            interleaved: false
        )!

        guard let conv = AVAudioConverter(from: inputFormat, to: targetFormat) else {
            throw NSError(domain: "AudioCapture", code: -2,
                          userInfo: [NSLocalizedDescriptionKey: "Cannot create audio format converter"])
        }
        self.converter = conv

        inputNode.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) {
            [weak self] buffer, _ in
            self?.processAudioBuffer(buffer)
        }

        engine.prepare()
        try engine.start()

        self.audioEngine = engine
        self.isCapturing = true
        print("[AudioCaptureManager] ✅ Started (\(inputFormat.sampleRate)Hz ch\(inputFormat.channelCount) → 16kHz mono Int16)")
    }

    func stopCapture() {
        guard isCapturing else { return }
        audioEngine?.inputNode.removeTap(onBus: 0)
        audioEngine?.stop()
        audioEngine = nil
        converter = nil
        isCapturing = false
        print("[AudioCaptureManager] ⏹ Stopped")
    }

    // MARK: - Audio Processing

    /// Resample native audio to 16kHz mono Float32, then convert to Int16 LE PCM.
    private func processAudioBuffer(_ buffer: AVAudioPCMBuffer) {
        guard let converter = converter, let onAudioData = onAudioData else { return }

        let ratio = 16000.0 / converter.inputFormat.sampleRate
        let outputFrameCapacity = AVAudioFrameCount(ceil(Double(buffer.frameLength) * ratio))
        guard outputFrameCapacity > 0,
              let outputBuffer = AVAudioPCMBuffer(
                  pcmFormat: converter.outputFormat,
                  frameCapacity: outputFrameCapacity
              ) else { return }

        var error: NSError?
        var hasInput = true
        let status = converter.convert(to: outputBuffer, error: &error) { _, outStatus in
            if hasInput {
                hasInput = false
                outStatus.pointee = .haveData
                return buffer
            }
            outStatus.pointee = .noDataNow
            return nil
        }

        guard status != .error, error == nil, outputBuffer.frameLength > 0 else { return }

        // Float32 → Int16 LE using doubao's non-symmetric scaling:
        //   negative samples × 32768, positive samples × 32767
        let floats = outputBuffer.floatChannelData![0]
        let count = Int(outputBuffer.frameLength)
        var pcm = Data(count: count * 2)
        pcm.withUnsafeMutableBytes { raw in
            let int16 = raw.bindMemory(to: Int16.self)
            for i in 0..<count {
                let s = floats[i]
                let v = s < 0 ? s * 32768.0 : s * 32767.0
                int16[i] = Int16(max(-32768, min(32767, v)))
            }
        }

        onAudioData(pcm)
    }
}
