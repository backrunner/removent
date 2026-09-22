// Synthetic UI only: does not capture the user's screen.
// swift scripts/readability_fixture.swift generate /tmp/removent-readability
// swift scripts/readability_fixture.swift recognize /tmp/removent-readability/*.png
import AppKit
import Vision

let args = Array(CommandLine.arguments.dropFirst())
guard args.count >= 2 else { fatalError("provide generate <directory> or recognize <images...>") }
if args[0] == "recognize" {
    for path in args.dropFirst() {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = ["zh-Hans", "en-US"]
        request.automaticallyDetectsLanguage = true
        request.usesLanguageCorrection = false
        try VNImageRequestHandler(url: URL(fileURLWithPath: path)).perform([request])
        let lines = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
        let result: [String: Any] = ["image": URL(fileURLWithPath: path).lastPathComponent, "lines": lines]
        let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
        print(String(data: data, encoding: .utf8)!)
    }
} else {
    guard args[0] == "generate" else { fatalError("unknown command") }
    let directory = URL(fileURLWithPath: args[1], isDirectory: true)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let width = 1920, height = 1080
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width, pixelsHigh: height,
                                 bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                 colorSpaceName: .deviceRGB, bytesPerRow: width * 4, bitsPerPixel: 32)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    NSColor.white.setFill()
    NSRect(x: 0, y: 0, width: width, height: height).fill()
    NSColor(calibratedWhite: 0.10, alpha: 1).setFill()
    NSRect(x: 960, y: 0, width: 960, height: height).fill()
    for column in 0...1 {
        let color = column == 0 ? NSColor.black : NSColor.white
        let x = CGFloat(40 + column * 960)
        let lines = [
            "Removent desktop readability / 远程桌面清晰度",
            "连接设置：中继地址、端口、凭据与受信任的设备",
            "网络较慢时保留文字细节，降低画面刷新频率。",
            "Host: workstation.example.com:443",
            "let status = connection.is_ready() && daemon.running;",
            "0123456789 Il1 O0 {} [] () != <= >= + - / _",
            "文件 编辑 视图 连接 帮助    保存 取消 重新连接"
        ]
        for (index, text) in lines.enumerated() {
            let size: CGFloat = index == 0 ? 22 : 14
            let font = index == 4 || index == 5
                ? NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
                : NSFont.systemFont(ofSize: size)
            (text as NSString).draw(at: NSPoint(x: x, y: CGFloat(1000 - index * 50)),
                                   withAttributes: [.font: font, .foregroundColor: color])
        }
        for (index, size) in [10, 12, 16, 20].enumerated() {
            ("字号 \(size) px — 状态正常 / Ready 1920 x 1080" as NSString)
                .draw(at: NSPoint(x: x, y: CGFloat(580 - index * 60)),
                      withAttributes: [.font: NSFont.systemFont(ofSize: CGFloat(size)), .foregroundColor: color])
        }
    }
    NSGraphicsContext.restoreGraphicsState()
    try bitmap.representation(using: .png, properties: [:])!.write(to: directory.appendingPathComponent("source.png"))
    let rgba = bitmap.bitmapData!
    var bgra = Data(count: width * height * 4)
    bgra.withUnsafeMutableBytes { bytes in
        let out = bytes.bindMemory(to: UInt8.self)
        for i in stride(from: 0, to: out.count, by: 4) {
            out[i] = rgba[i + 2]; out[i + 1] = rgba[i + 1]
            out[i + 2] = rgba[i]; out[i + 3] = 255
        }
    }
    try bgra.write(to: directory.appendingPathComponent("source.bgra"))
}
