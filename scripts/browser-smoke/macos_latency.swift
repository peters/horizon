import AppKit
import CoreGraphics
import CoreMedia
import Foundation
import ScreenCaptureKit

// Measures native input-to-visible-window latency for one exact PID/window.

struct Region {
    let x: Int
    let y: Int
    let width: Int
    let height: Int
}

enum LatencyError: Error {
    case usage
    case unavailable(String)
    case timeout(Int)
}

func argument<T: LosslessStringConvertible>(_ index: Int, as _: T.Type = T.self) throws -> T {
    guard CommandLine.arguments.indices.contains(index), let value = T(CommandLine.arguments[index]) else {
        throw LatencyError.usage
    }
    return value
}

func keyEvent(_ code: CGKeyCode, down: Bool) throws -> CGEvent {
    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down) else {
        throw LatencyError.unavailable("could not create key event")
    }
    return event
}

func postKey(_ code: CGKeyCode, flags: CGEventFlags = []) throws {
    let down = try keyEvent(code, down: true)
    let up = try keyEvent(code, down: false)
    down.flags = flags
    up.flags = flags
    down.post(tap: .cghidEventTap)
    usleep(10_000)
    up.post(tap: .cghidEventTap)
}

func postSelectAll() throws {
    let commandDown = try keyEvent(55, down: true)
    let commandUp = try keyEvent(55, down: false)
    let selectDown = try keyEvent(0, down: true)
    let selectUp = try keyEvent(0, down: false)
    commandDown.flags = .maskCommand
    selectDown.flags = .maskCommand
    selectUp.flags = .maskCommand
    commandDown.post(tap: .cghidEventTap)
    selectDown.post(tap: .cghidEventTap)
    usleep(10_000)
    selectUp.post(tap: .cghidEventTap)
    commandUp.post(tap: .cghidEventTap)
    usleep(10_000)
}

func postText(_ text: String) throws {
    let down = try keyEvent(0, down: true)
    let up = try keyEvent(0, down: false)
    var units = Array(text.utf16)
    down.keyboardSetUnicodeString(stringLength: units.count, unicodeString: &units)
    down.post(tap: .cghidEventTap)
    usleep(10_000)
    up.post(tap: .cghidEventTap)
}

func postClick(x: Double, y: Double) throws {
    let point = CGPoint(x: x, y: y)
    guard let down = CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left),
          let up = CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)
    else { throw LatencyError.unavailable("could not create mouse event") }
    down.post(tap: .cghidEventTap)
    usleep(35_000)
    up.post(tap: .cghidEventTap)
}

final class FrameObserver: NSObject, SCStreamOutput, @unchecked Sendable {
    private let lock = NSLock()
    private let region: Region
    private var sequence = 0
    private var value: UInt64 = 0
    private var timestamp: UInt64 = 0

    init(region: Region) {
        self.region = region
    }

    func snapshot() -> (sequence: Int, value: UInt64, timestamp: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        return (sequence, value, timestamp)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, buffer.isValid, let image = CMSampleBufferGetImageBuffer(buffer) else { return }
        CVPixelBufferLockBaseAddress(image, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(image, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(image) else { return }
        let imageWidth = CVPixelBufferGetWidth(image)
        let imageHeight = CVPixelBufferGetHeight(image)
        let stride = CVPixelBufferGetBytesPerRow(image)
        let x0 = max(0, min(region.x, imageWidth - 1))
        let y0 = max(0, min(region.y, imageHeight - 1))
        let x1 = max(x0 + 1, min(x0 + region.width, imageWidth))
        let y1 = max(y0 + 1, min(y0 + region.height, imageHeight))
        let bytes = base.assumingMemoryBound(to: UInt8.self)
        var hash: UInt64 = 14_695_981_039_346_656_037
        for y in y0..<y1 {
            let row = bytes + y * stride
            for x in x0..<x1 {
                let pixel = row + x * 4
                for channel in 0..<4 {
                    hash ^= UInt64(pixel[channel])
                    hash &*= 1_099_511_628_211
                }
            }
        }
        lock.lock()
        sequence += 1
        value = hash
        timestamp = DispatchTime.now().uptimeNanoseconds
        lock.unlock()
    }
}

func waitForChange(_ observer: FrameObserver, after baseline: (sequence: Int, value: UInt64, timestamp: UInt64), sample: Int) throws -> UInt64 {
    let deadline = DispatchTime.now().uptimeNanoseconds + 2_000_000_000
    while DispatchTime.now().uptimeNanoseconds < deadline {
        let current = observer.snapshot()
        if current.sequence > baseline.sequence && current.value != baseline.value { return current.timestamp }
        usleep(1_000)
    }
    throw LatencyError.timeout(sample)
}

func activate(_ app: NSRunningApplication, pid: pid_t) throws {
    app.activate(options: [.activateAllWindows])
    usleep(500_000)
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
        throw LatencyError.unavailable("could not make candidate frontmost")
    }
}

func result(mode: String, samples: [Double]) throws -> Data {
    let ordered = samples.sorted()
    let payload: [String: Any] = [
        "count": samples.count,
        "max_ms": ordered.last ?? 0,
        "median_ms": (ordered[9] + ordered[10]) / 2,
        "mode": mode,
        "p95_ms": ordered[18],
        "samples_ms": samples,
    ]
    return try JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted, .sortedKeys])
}

@main
struct MacOSLatencySmoke {
    static func main() async throws {
        _ = NSApplication.shared
        guard CommandLine.arguments.count == 11 || CommandLine.arguments.count == 12 else {
            throw LatencyError.usage
        }
        let pid: pid_t = try argument(1)
        let windowID: CGWindowID = try argument(2)
        let mode = CommandLine.arguments[3]
        let targetX: Double = try argument(4)
        let targetY: Double = try argument(5)
        let region = Region(x: try argument(6), y: try argument(7), width: try argument(8), height: try argument(9))
        let output = URL(fileURLWithPath: CommandLine.arguments[10])
        let baseURL = CommandLine.arguments.indices.contains(11) ? CommandLine.arguments[11] : ""
        guard (mode == "input" && CommandLine.arguments.count == 11
                || mode == "navigation" && CommandLine.arguments.count == 12),
              let app = NSRunningApplication(processIdentifier: pid)
        else { throw LatencyError.usage }

        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let window = content.windows.first(where: {
            $0.windowID == windowID && $0.owningApplication?.processID == pid
        }) else { throw LatencyError.unavailable("window does not belong to candidate PID") }

        let observer = FrameObserver(region: region)
        let configuration = SCStreamConfiguration()
        configuration.width = Int(window.frame.width)
        configuration.height = Int(window.frame.height)
        configuration.showsCursor = false
        configuration.minimumFrameInterval = CMTime(value: 1, timescale: 120)
        configuration.queueDepth = 3
        let stream = SCStream(
            filter: SCContentFilter(desktopIndependentWindow: window),
            configuration: configuration,
            delegate: nil
        )
        try stream.addStreamOutput(observer, type: .screen, sampleHandlerQueue: DispatchQueue(label: "horizon-smoke-latency"))
        try await stream.startCapture()
        try activate(app, pid: pid)

        var samples: [Double] = []
        if mode == "input" {
            try postClick(x: targetX, y: targetY)
            usleep(250_000)
            let keys: [CGKeyCode] = [29, 18, 19, 20, 21, 23, 22, 26, 28, 25]
            for index in 0..<20 {
                let baseline = observer.snapshot()
                let started = DispatchTime.now().uptimeNanoseconds
                try postKey(keys[index % keys.count])
                let finished = try waitForChange(observer, after: baseline, sample: index)
                samples.append(Double(finished - started) / 1_000_000)
                usleep(100_000)
            }
        } else {
            for index in 0..<20 {
                try postClick(x: targetX, y: targetY)
                usleep(200_000)
                try postSelectAll()
                try postText("\(baseURL)/\(index.isMultiple(of: 2) ? "next.html" : "alternate.html")")
                let baseline = observer.snapshot()
                let started = DispatchTime.now().uptimeNanoseconds
                try postKey(36)
                let finished = try waitForChange(observer, after: baseline, sample: index)
                samples.append(Double(finished - started) / 1_000_000)
                usleep(100_000)
            }
        }
        try await stream.stopCapture()
        let encoded = try result(mode: mode, samples: samples)
        try encoded.write(to: output)
        print(String(decoding: encoded, as: UTF8.self))
    }
}
