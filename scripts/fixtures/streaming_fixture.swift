// Compile: swiftc -O streaming_fixture.swift -o streaming-fixture
// Run in the logged-in GUI session: streaming-fixture [seconds] [--audio]
// Deterministic moving GPU content. Output counts source drawable presentations,
// not remote reception; the optional quiet tone is not an acoustic latency test.
import AppKit
import MetalKit
import AVFoundation

final class Fixture: NSObject, NSApplicationDelegate, MTKViewDelegate {
    var window: NSWindow!
    var view: MTKView!
    var queue: MTLCommandQueue!
    var pipeline: MTLRenderPipelineState!
    var audio: AVAudioEngine?
    let started = CACurrentMediaTime()
    let statsLock = NSLock()
    var presented = 0
    var lastPresented = 0.0
    var gaps: [Double] = []
    var timer: Timer?
    var duration = 180.0

    func applicationDidFinishLaunching(_ notification: Notification) {
        duration = CommandLine.arguments.dropFirst().compactMap(Double.init).first ?? 180
        let screen = NSScreen.main!
        let device = MTLCreateSystemDefaultDevice()!
        view = MTKView(frame: screen.frame, device: device)
        view.preferredFramesPerSecond = 60
        view.colorPixelFormat = .bgra8Unorm
        view.framebufferOnly = true
        view.delegate = self
        queue = device.makeCommandQueue()!
        let library = try! device.makeLibrary(source: """
        #include <metal_stdlib>
        using namespace metal;
        struct Vertex { float4 position [[position]]; float2 uv; };
        vertex Vertex vertex_main(uint id [[vertex_id]]) {
            float2 p = float2((id << 1) & 2, id & 2);
            return {float4(p * 2 - 1, 0, 1), p};
        }
        fragment float4 fragment_main(Vertex v [[stage_in]], constant float &t [[buffer(0)]]) {
            float2 p = v.uv;
            float grid = step(0.93, fract((p.x + t * 0.12) * 24))
                       + step(0.93, fract((p.y + t * 0.06) * 14));
            float ball = 1 - smoothstep(0.055, 0.06,
                length((p - float2(0.5 + 0.38*sin(t), 0.5 + 0.35*cos(t*1.3))) * float2(1.7778, 1)));
            float3 color = float3(0.07 + p.x*0.2, 0.1 + p.y*0.2, 0.2);
            color += min(grid, 1.0) * float3(0.12, 0.28, 0.35);
            color = mix(color, float3(1, 0.75, 0.15), ball);
            // Each presented source frame changes broad image areas continuously.
            color += 0.04 * sin(float3(2, 3, 4) * t + p.xyx * 9);
            return float4(color, 1);
        }
        """, options: nil)
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: "vertex_main")
        descriptor.fragmentFunction = library.makeFunction(name: "fragment_main")
        descriptor.colorAttachments[0].pixelFormat = .bgra8Unorm
        pipeline = try! device.makeRenderPipelineState(descriptor: descriptor)
        window = NSWindow(contentRect: screen.frame, styleMask: [.borderless],
                          backing: .buffered, defer: false)
        window.title = "Removent streaming fixture"
        window.contentView = view
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        if CommandLine.arguments.contains("--audio") { startAudio() }
        timer = Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { [weak self] _ in
            self?.report()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + duration) { [weak self] in
            self?.report()
            self?.audio?.stop()
            NSApp.terminate(nil)
        }
    }

    func startAudio() {
        let engine = AVAudioEngine()
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        var phase = 0.0
        let source = AVAudioSourceNode(format: format) { _, _, count, buffers in
            let list = UnsafeMutableAudioBufferListPointer(buffers)
            for frame in 0..<Int(count) {
                let sample = Float(sin(phase) * 0.01)
                phase += 2 * Double.pi * 440 / 48_000
                if phase >= 2 * Double.pi { phase -= 2 * Double.pi }
                for buffer in list {
                    buffer.mData!.assumingMemoryBound(to: Float.self)[frame] = sample
                }
            }
            return noErr
        }
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: format)
        do { try engine.start(); audio = engine }
        catch { print("audio_start_failed: \(error)") }
    }

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

    func draw(in view: MTKView) {
        guard let descriptor = view.currentRenderPassDescriptor,
              let drawable = view.currentDrawable,
              let command = queue.makeCommandBuffer(),
              let encoder = command.makeRenderCommandEncoder(descriptor: descriptor) else { return }
        var elapsed = Float(CACurrentMediaTime() - started)
        encoder.setRenderPipelineState(pipeline)
        encoder.setFragmentBytes(&elapsed, length: MemoryLayout<Float>.size, index: 0)
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        encoder.endEncoding()
        drawable.addPresentedHandler { [weak self] actual in
            guard let self, actual.presentedTime > 0 else { return }
            self.statsLock.lock()
            if self.lastPresented > 0 { self.gaps.append((actual.presentedTime - self.lastPresented) * 1000) }
            self.lastPresented = actual.presentedTime
            self.presented += 1
            self.statsLock.unlock()
        }
        command.present(drawable)
        command.commit()
    }

    func report() {
        statsLock.lock()
        let count = presented
        let sorted = gaps.sorted()
        presented = 0
        gaps.removeAll(keepingCapacity: true)
        statsLock.unlock()
        func percentile(_ p: Double) -> Double {
            sorted.isEmpty ? 0 : sorted[min(sorted.count - 1, Int(ceil(Double(sorted.count) * p)) - 1)]
        }
        let data: [String: Any] = ["elapsed_s": CACurrentMediaTime() - started,
            "source_presented": count, "gap_p95_ms": percentile(0.95),
            "gap_p99_ms": percentile(0.99), "gap_max_ms": sorted.last ?? 0]
        let json = try! JSONSerialization.data(withJSONObject: data, options: [.sortedKeys])
        print(String(data: json, encoding: .utf8)!)
        fflush(stdout)
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let fixture = Fixture()
app.delegate = fixture
app.run()
