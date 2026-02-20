import AVFoundation
import CoreGraphics
import CoreMedia
import Darwin
import Foundation
import ScreenCaptureKit

enum CaptureError: LocalizedError {
    case invalidArguments
    case missingOutputPath
    case unsupportedMacOSVersion
    case screenPermissionDenied
    case noDisplayAvailable
    case writerConfigurationFailed
    case noAudioCaptured

    var errorDescription: String? {
        switch self {
        case .invalidArguments:
            return "Usage: system-audio-capture record --output <path>"
        case .missingOutputPath:
            return "Missing required --output <path> argument."
        case .unsupportedMacOSVersion:
            return "System audio capture requires macOS 13 or newer."
        case .screenPermissionDenied:
            return "Screen Recording permission is required. Enable it in System Settings > Privacy & Security > Screen Recording."
        case .noDisplayAvailable:
            return "No display is available for system audio capture."
        case .writerConfigurationFailed:
            return "Failed to configure audio writer."
        case .noAudioCaptured:
            return "No shared audio was captured."
        }
    }
}

struct CLIArguments {
    let outputPath: String
}

func parseArguments() throws -> CLIArguments {
    let args = CommandLine.arguments
    guard args.count >= 2, args[1] == "record" else {
        throw CaptureError.invalidArguments
    }

    guard let outputFlagIndex = args.firstIndex(of: "--output") else {
        throw CaptureError.missingOutputPath
    }

    let outputIndex = outputFlagIndex + 1
    guard outputIndex < args.count else {
        throw CaptureError.missingOutputPath
    }

    return CLIArguments(outputPath: args[outputIndex])
}

func ensureScreenRecordingPermission() throws {
    if CGPreflightScreenCaptureAccess() {
        return
    }

    _ = CGRequestScreenCaptureAccess()
    if !CGPreflightScreenCaptureAccess() {
        throw CaptureError.screenPermissionDenied
    }
}

@available(macOS 13.0, *)
final class StreamAudioWriter: NSObject, SCStreamOutput, SCStreamDelegate {
    private let writer: AVAssetWriter
    private let writerInput: AVAssetWriterInput
    private var started = false
    private(set) var hasAudio = false
    private(set) var streamError: Error?

    init(outputURL: URL) throws {
        if FileManager.default.fileExists(atPath: outputURL.path) {
            try? FileManager.default.removeItem(at: outputURL)
        }

        writer = try AVAssetWriter(outputURL: outputURL, fileType: .wav)

        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatLinearPCM,
            AVSampleRateKey: 16_000,
            AVNumberOfChannelsKey: 1,
            AVLinearPCMBitDepthKey: 16,
            AVLinearPCMIsFloatKey: false,
            AVLinearPCMIsBigEndianKey: false,
            AVLinearPCMIsNonInterleaved: false,
        ]

        writerInput = AVAssetWriterInput(mediaType: .audio, outputSettings: settings)
        writerInput.expectsMediaDataInRealTime = true

        guard writer.canAdd(writerInput) else {
            throw CaptureError.writerConfigurationFailed
        }

        writer.add(writerInput)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of outputType: SCStreamOutputType) {
        guard outputType == .audio else {
            return
        }

        guard streamError == nil else {
            return
        }

        guard CMSampleBufferDataIsReady(sampleBuffer) else {
            return
        }

        let timestamp = CMSampleBufferGetPresentationTimeStamp(sampleBuffer)
        if !started {
            if !writer.startWriting() {
                streamError = writer.error ?? NSError(domain: "system-audio-capture", code: 1)
                return
            }
            writer.startSession(atSourceTime: timestamp)
            started = true
        }

        guard writerInput.isReadyForMoreMediaData else {
            return
        }

        if writerInput.append(sampleBuffer) {
            hasAudio = true
            return
        }

        streamError = writer.error ?? NSError(domain: "system-audio-capture", code: 2)
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        streamError = error
    }

    func finish() async throws {
        if let error = streamError {
            throw error
        }

        guard started else {
            throw CaptureError.noAudioCaptured
        }

        writerInput.markAsFinished()

        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            writer.finishWriting {
                if let error = self.streamError ?? self.writer.error {
                    continuation.resume(throwing: error)
                    return
                }
                continuation.resume()
            }
        }

        if !hasAudio {
            throw CaptureError.noAudioCaptured
        }
    }
}

@available(macOS 13.0, *)
final class SystemAudioRecorder {
    private let outputURL: URL
    private let writer: StreamAudioWriter
    private let stopSignal = DispatchSemaphore(value: 0)
    private var stream: SCStream?

    init(outputPath: String) throws {
        outputURL = URL(fileURLWithPath: outputPath)
        writer = try StreamAudioWriter(outputURL: outputURL)
    }

    func runUntilStdinClosed() async throws {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let display = content.displays.first else {
            throw CaptureError.noDisplayAvailable
        }

        let filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])
        let config = SCStreamConfiguration()
        config.capturesAudio = true
        config.excludesCurrentProcessAudio = false
        config.sampleRate = 16_000
        config.channelCount = 1
        config.queueDepth = 3
        config.minimumFrameInterval = CMTime(value: 1, timescale: 60)
        config.width = display.width
        config.height = display.height

        let sampleQueue = DispatchQueue(label: "echo-scribe.system-audio")
        let stream = SCStream(filter: filter, configuration: config, delegate: writer)
        self.stream = stream

        try stream.addStreamOutput(writer, type: .audio, sampleHandlerQueue: sampleQueue)
        try await stream.startCapture()

        startStdinWatcher()
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.global(qos: .utility).async {
                self.stopSignal.wait()
                continuation.resume()
            }
        }

        try await stream.stopCapture()
        try await writer.finish()
    }

    private func startStdinWatcher() {
        DispatchQueue.global(qos: .utility).async {
            var buffer = [UInt8](repeating: 0, count: 1)
            while true {
                let bytesRead = Darwin.read(STDIN_FILENO, &buffer, 1)
                if bytesRead <= 0 {
                    break
                }
            }
            self.stopSignal.signal()
        }
    }
}

@main
struct SystemAudioCaptureMain {
    static func main() async {
        do {
            guard #available(macOS 13.0, *) else {
                throw CaptureError.unsupportedMacOSVersion
            }

            let cli = try parseArguments()
            try ensureScreenRecordingPermission()

            let recorder = try SystemAudioRecorder(outputPath: cli.outputPath)
            try await recorder.runUntilStdinClosed()
            exit(0)
        } catch {
            let message = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            fputs("ERROR: \(message)\n", stderr)
            exit(1)
        }
    }
}
