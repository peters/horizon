import AppKit
import CoreGraphics
import Foundation

// Exact-PID native input, window selection, and motion tracing for macOS smoke.

enum SmokeError: Error {
    case usage(String)
    case unavailable(String)
}

func argument<T: LosslessStringConvertible>(_ index: Int, as _: T.Type = T.self) throws -> T {
    guard CommandLine.arguments.indices.contains(index), let value = T(CommandLine.arguments[index]) else {
        throw SmokeError.usage("invalid or missing argument \(index)")
    }
    return value
}

func event(_ type: CGEventType, _ point: CGPoint, _ button: CGMouseButton) throws -> CGEvent {
    guard let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: button)
    else { throw SmokeError.unavailable("could not create mouse event") }
    return event
}

func keyEvent(_ code: CGKeyCode, down: Bool) throws -> CGEvent {
    guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)
    else { throw SmokeError.unavailable("could not create key event") }
    return event
}

func post(_ event: CGEvent, delay: useconds_t = 35_000) {
    event.post(tap: .cghidEventTap)
    usleep(delay)
}

struct OwnedWindow {
    let id: CGWindowID
    let title: String
    let bounds: CGRect
    let onscreen: Bool
}

func number(_ value: Any?) -> NSNumber? {
    value as? NSNumber
}

func ownedWindow(_ info: [String: Any], pid: pid_t) -> OwnedWindow? {
    guard number(info[kCGWindowOwnerPID as String])?.int32Value == pid,
          number(info[kCGWindowLayer as String])?.intValue == 0,
          let id = number(info[kCGWindowNumber as String]).map({ CGWindowID($0.uint32Value) }),
          let values = info[kCGWindowBounds as String] as? [String: Any],
          let x = number(values["X"])?.doubleValue,
          let y = number(values["Y"])?.doubleValue,
          let width = number(values["Width"])?.doubleValue,
          let height = number(values["Height"])?.doubleValue,
          width > 100,
          height > 100
    else { return nil }
    return OwnedWindow(
        id: id,
        title: info[kCGWindowName as String] as? String ?? "",
        bounds: CGRect(x: x, y: y, width: width, height: height),
        onscreen: (info[kCGWindowIsOnscreen as String] as? Bool) ?? false
    )
}

func ownedWindows(pid: pid_t) -> [OwnedWindow] {
    let info = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    return info.compactMap { ownedWindow($0, pid: pid) }
}

func window(pid: pid_t, kind: String) throws -> OwnedWindow {
    let candidates = ownedWindows(pid: pid)
    let root = candidates.first { $0.title == "Horizon" && $0.onscreen }
    let detached = candidates.first { !$0.title.isEmpty && $0.title != "Horizon" && $0.onscreen }
    guard let selected = switch kind {
    case "root": root ?? candidates.first(where: \.onscreen)
    case "detached": detached
    case "any": candidates.first(where: \.onscreen)
    default: nil
    } else { throw SmokeError.unavailable("no \(kind) window for PID \(pid)") }
    return selected
}

func activate(pid: pid_t) throws {
    guard let app = NSRunningApplication(processIdentifier: pid) else {
        throw SmokeError.unavailable("PID \(pid) is not running")
    }
    app.activate(options: [.activateAllWindows])
    usleep(300_000)
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
        throw SmokeError.unavailable("could not make PID \(pid) frontmost")
    }
}

func printWindows(pid: pid_t) throws {
    let info = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    let selected = info.filter {
        number($0[kCGWindowOwnerPID as String])?.int32Value == pid
            && number($0[kCGWindowLayer as String])?.intValue == 0
    }
    let data = try JSONSerialization.data(withJSONObject: selected, options: [.prettyPrinted, .sortedKeys])
    print(String(decoding: data, as: UTF8.self))
}

func trace(pid: pid_t, kind: String, samples: Int, intervalMillis: UInt32) throws {
    let id = try window(pid: pid, kind: kind).id
    print("elapsed_ms,x,y,width,height")
    let started = DispatchTime.now().uptimeNanoseconds
    for _ in 0..<samples {
        let info = CGWindowListCopyWindowInfo(.optionIncludingWindow, id) as? [[String: Any]] ?? []
        guard let current = info.first.flatMap({ ownedWindow($0, pid: pid) }) else {
            throw SmokeError.unavailable("window \(id) disappeared during trace")
        }
        let elapsed = Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
        print(String(
            format: "%.3f,%.1f,%.1f,%.1f,%.1f",
            elapsed,
            current.bounds.minX,
            current.bounds.minY,
            current.bounds.width,
            current.bounds.height
        ))
        usleep(intervalMillis * 1_000)
    }
}

func mouseButton(_ name: String) throws -> (CGMouseButton, CGEventType, CGEventType) {
    switch name {
    case "left": (.left, .leftMouseDown, .leftMouseUp)
    case "right": (.right, .rightMouseDown, .rightMouseUp)
    case "middle": (.center, .otherMouseDown, .otherMouseUp)
    default: throw SmokeError.usage("button must be left, right, or middle")
    }
}

func click(point: CGPoint, buttonName: String, count: Int = 1) throws {
    let (button, downType, upType) = try mouseButton(buttonName)
    for clickCount in 1...count {
        let down = try event(downType, point, button)
        let up = try event(upType, point, button)
        down.setIntegerValueField(.mouseEventClickState, value: Int64(clickCount))
        up.setIntegerValueField(.mouseEventClickState, value: Int64(clickCount))
        post(down)
        post(up)
        usleep(45_000)
    }
}

func drag(from start: CGPoint, to end: CGPoint, steps: Int, shiftRelease: Bool) throws {
    post(try event(.mouseMoved, start, .left))
    post(try event(.leftMouseDown, start, .left))
    for step in 1...max(steps, 1) {
        let fraction = Double(step) / Double(max(steps, 1))
        let point = CGPoint(x: start.x + (end.x - start.x) * fraction, y: start.y + (end.y - start.y) * fraction)
        post(try event(.leftMouseDragged, point, .left))
    }
    if shiftRelease { post(try keyEvent(56, down: true)) }
    let up = try event(.leftMouseUp, end, .left)
    if shiftRelease { up.flags = .maskShift }
    post(up)
    if shiftRelease { post(try keyEvent(56, down: false)) }
}

func flags(named names: String) -> (CGEventFlags, [CGKeyCode]) {
    names.split(separator: ",").reduce(into: (CGEventFlags(), [CGKeyCode]())) { result, name in
        switch name {
        case "command": result.0.insert(.maskCommand); result.1.append(55)
        case "shift": result.0.insert(.maskShift); result.1.append(56)
        case "control": result.0.insert(.maskControl); result.1.append(59)
        case "option": result.0.insert(.maskAlternate); result.1.append(58)
        case "fn": result.0.insert(.maskSecondaryFn); result.1.append(63)
        default: break
        }
    }
}

func pressKey(code: CGKeyCode, modifiers: String, repeatCount: Int = 0) throws {
    let (eventFlags, modifierCodes) = flags(named: modifiers)
    for modifier in modifierCodes {
        let down = try keyEvent(modifier, down: true)
        down.flags = eventFlags
        post(down)
    }
    let down = try keyEvent(code, down: true)
    down.flags = eventFlags
    post(down)
    for _ in 0..<repeatCount {
        let repeated = try keyEvent(code, down: true)
        repeated.flags = eventFlags
        repeated.setIntegerValueField(.keyboardEventAutorepeat, value: 1)
        post(repeated)
    }
    let up = try keyEvent(code, down: false)
    up.flags = eventFlags
    post(up)
    for modifier in modifierCodes.reversed() { post(try keyEvent(modifier, down: false)) }
}

func typeText(_ text: String) throws {
    for character in text {
        let down = try keyEvent(0, down: true)
        let up = try keyEvent(0, down: false)
        var units = Array(String(character).utf16)
        down.keyboardSetUnicodeString(stringLength: units.count, unicodeString: &units)
        post(down)
        post(up)
    }
}

func simultaneousButtons(at start: CGPoint) throws {
    let end = CGPoint(x: start.x + 25, y: start.y)
    post(try event(.leftMouseDown, start, .left))
    post(try event(.rightMouseDown, start, .right))
    post(try event(.leftMouseDragged, end, .left))
    post(try event(.rightMouseUp, end, .right))
    post(try event(.leftMouseUp, end, .left))
}

func closeWindow(pid: pid_t, kind: String) throws {
    let target = try window(pid: pid, kind: kind)
    try click(
        point: CGPoint(x: target.bounds.minX + 14, y: target.bounds.minY + 14),
        buttonName: "left"
    )
}

@main
struct MacOSNativeSmoke {
    static func main() throws {
        guard CommandLine.arguments.count >= 4 else {
            throw SmokeError.usage("macos_native <pid> <root|detached|any> <command> [args]")
        }
        let pid: pid_t = try argument(1)
        let kind = CommandLine.arguments[2]
        guard ["root", "detached", "any"].contains(kind) else {
            throw SmokeError.usage("window kind must be root, detached, or any")
        }
        let command = CommandLine.arguments[3]
        if command == "windows" { try printWindows(pid: pid); return }
        if command == "trace" {
            try trace(pid: pid, kind: kind, samples: argument(4), intervalMillis: argument(5))
            return
        }
        try activate(pid: pid)
        switch command {
        case "close": try closeWindow(pid: pid, kind: kind)
        case "click", "double", "right":
            let x: Double = try argument(4)
            let y: Double = try argument(5)
            let point = CGPoint(x: x, y: y)
            try click(point: point, buttonName: command == "right" ? "right" : "left", count: command == "double" ? 2 : 1)
        case "drag":
            let startX: Double = try argument(4)
            let startY: Double = try argument(5)
            let endX: Double = try argument(6)
            let endY: Double = try argument(7)
            let start = CGPoint(x: startX, y: startY)
            let end = CGPoint(x: endX, y: endY)
            let steps: Int = CommandLine.arguments.indices.contains(8) ? try argument(8) : 20
            try drag(from: start, to: end, steps: steps, shiftRelease: CommandLine.arguments.last == "shift-release")
        case "scroll":
            let x: Double = try argument(4)
            let y: Double = try argument(5)
            let point = CGPoint(x: x, y: y)
            let dx: Int32 = try argument(6)
            let dy: Int32 = try argument(7)
            guard let wheel = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0)
            else { throw SmokeError.unavailable("could not create scroll event") }
            wheel.location = point
            post(wheel)
        case "key", "repeat":
            let code: CGKeyCode = try argument(4)
            let modifiers = CommandLine.arguments.indices.contains(5) ? CommandLine.arguments[5] : ""
            let repeats: Int = command == "repeat" ? try argument(6) : 0
            try pressKey(code: code, modifiers: modifiers, repeatCount: repeats)
        case "text":
            guard CommandLine.arguments.indices.contains(4) else { throw SmokeError.usage("text is required") }
            try typeText(CommandLine.arguments[4])
        case "buttons":
            let x: Double = try argument(4)
            let y: Double = try argument(5)
            try simultaneousButtons(at: CGPoint(x: x, y: y))
        default: throw SmokeError.usage("unknown command \(command)")
        }
    }
}
