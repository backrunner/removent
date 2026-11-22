// Replay decoded BGRA frames at their saved source timestamps in a local Metal window.
// swiftc -O scripts/fixtures/region_replay.swift -o /tmp/region-replay
// /tmp/region-replay <width> <height> <clip.bgra>  (also reads clip.pts)
import AppKit
import Metal
import QuartzCore

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

let arguments = Array(CommandLine.arguments.dropFirst())
guard arguments.count == 3, let width = Int(arguments[0]), let height = Int(arguments[1]),
      (1...8192).contains(width), (1...8192).contains(height) else {
    fail("usage: region-replay width height clip.bgra")
}
let file = URL(fileURLWithPath: arguments[2])
let rows = try String(contentsOf: file.deletingPathExtension().appendingPathExtension("pts"),
                      encoding: .utf8).split(whereSeparator: \.isNewline)
let pts = rows.compactMap { Int64($0) }
let pixels = try Data(contentsOf: file, options: .alwaysMapped)
let frameBytes = width * height * 4
guard pts.count == rows.count, let first = pts.first, first >= 0,
      zip(pts, pts.dropFirst()).allSatisfy({ $0.0 < $0.1 }),
      pixels.count % frameBytes == 0, pixels.count / frameBytes == pts.count else {
    fail("expected a nonempty, strictly increasing PTS list and one full BGRA frame per entry")
}
guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue() else {
    fail("Metal unavailable")
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let surface = CAMetalLayer()
surface.device = device
surface.pixelFormat = .bgra8Unorm
surface.framebufferOnly = false
let view = NSView(frame: NSRect(x: 0, y: 0, width: max(1, width / 2), height: max(1, height / 2)))
view.wantsLayer = true
view.layer = surface
let window = NSWindow(contentRect: view.frame, styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Removent region compression replay"
window.contentView = view
window.center()
window.orderFrontRegardless()
surface.drawableSize = CGSize(width: width, height: height)

var presented: [Int: Double] = [:]
var submitted = 0
var skipped = 0
var undisplayed = 0
var endOfClip = false
var startTime = DispatchTime.now()

func finishIfReady() {
    guard endOfClip, presented.count + undisplayed == submitted else { return }
    // Sort by actual display time. Only genuinely presented new frames have a timestamp.
    let times = presented.values.sorted()
    let intervals = zip(times, times.dropFirst()).map { ($1 - $0) * 1000 }.sorted()
    func percentile(_ p: Double) -> Any {
        if intervals.isEmpty { return NSNull() }
        return intervals[Int(ceil(p * Double(intervals.count))) - 1]
    }
    let record: [String: Any] = [
        "clip": file.deletingPathExtension().lastPathComponent, "frames_in_file": pts.count,
        "submitted": submitted, "presented": presented.count, "skipped_late": skipped,
        "no_display_time": undisplayed,
        "interval_p95_ms": percentile(0.95), "interval_p99_ms": percentile(0.99)
    ]
    let data = try! JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
    print(String(decoding: data, as: UTF8.self))
    if presented.isEmpty { fail("no positive drawable presentation timestamps") }
    app.terminate(nil)
}

func deadline(_ index: Int) -> DispatchTime {
    startTime + .microseconds(Int(pts[index] - first))
}

func submit(_ index: Int) {
    guard let drawable = surface.nextDrawable(), let command = queue.makeCommandBuffer(),
          let blit = command.makeBlitCommandEncoder() else { fail("unable to acquire drawable") }
    let buffer = pixels.withUnsafeBytes {
        device.makeBuffer(bytes: $0.baseAddress!.advanced(by: index * frameBytes),
                          length: frameBytes, options: .storageModeShared)
    }
    guard let buffer else { fail("unable to stage BGRA frame") }
    blit.copy(from: buffer, sourceOffset: 0, sourceBytesPerRow: width * 4,
              sourceBytesPerImage: frameBytes, sourceSize: MTLSize(width: width, height: height, depth: 1),
              to: drawable.texture, destinationSlice: 0, destinationLevel: 0,
              destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
    blit.endEncoding()
    drawable.addPresentedHandler { result in
        let time = result.presentedTime
        DispatchQueue.main.async {
            if time > 0 { presented[index] = time } else { undisplayed += 1 }
            finishIfReady()
        }
    }
    submitted += 1
    command.present(drawable)
    command.commit()
}

func schedule(_ next: Int) {
    guard next < pts.count else {
        endOfClip = true
        finishIfReady()
        DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(4)) {
            if presented.count + undisplayed < submitted { fail("presentation callback timed out") }
        }
        return
    }
    DispatchQueue.main.asyncAfter(deadline: deadline(next)) {
        var index = next
        let now = DispatchTime.now()
        while index + 1 < pts.count && deadline(index + 1) <= now { index += 1 }
        skipped += index - next
        autoreleasepool { submit(index) }
        schedule(index + 1)
    }
}

DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(300)) {
    startTime = .now()
    schedule(0)
}
app.run()
