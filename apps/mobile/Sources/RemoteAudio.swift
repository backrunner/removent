import AVFoundation

// Retained by the render block, independently of the UI model and Rust engine.
// Its only mutable access is the serial AVAudioSourceNode callback.
private final class AudioRenderContext: @unchecked Sendable {
    let handle: OpaquePointer
    init(_ handle: OpaquePointer) { self.handle = handle }
    deinit { rm_audio_renderer_free(handle) }
}

@MainActor
final class RemoteAudio {
    private let engine = AVAudioEngine()
    private var source: AVAudioSourceNode?
    private var epoch = 0
    private let sessionQueue = DispatchQueue(label: "com.alkinum.removent.audio-session")
    var muted = false { didSet { engine.mainMixerNode.outputVolume = muted ? 0 : 1 } }

    func start(rate: Double, channels: Int, handle: OpaquePointer, generation: UInt64) async throws {
        stop()
        let current = epoch
        guard rate == 48000, channels == 2,
            let format = AVAudioFormat(standardFormatWithSampleRate:rate, channels:2),
            let renderer = rm_audio_renderer_create(handle, generation) else {
            throw MobileError.message("Unsupported or expired audio session")
        }
        let context = AudioRenderContext(renderer)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            sessionQueue.async {
                do {
                    let session = AVAudioSession.sharedInstance()
                    try session.setCategory(.playback, mode:.default)
                    try session.setPreferredIOBufferDuration(0.005)
                    try session.setActive(true)
                    continuation.resume()
                } catch { continuation.resume(throwing: error) }
            }
        }
        guard current == epoch else { return }
        let node = AVAudioSourceNode(format:format) { _, _, frameCount, buffers in
            let output = UnsafeMutableAudioBufferListPointer(buffers)
            guard output.count == 2,
                output[0].mDataByteSize >= frameCount * 4,
                output[1].mDataByteSize >= frameCount * 4,
                let left = output[0].mData?.assumingMemoryBound(to:Float.self),
                let right = output[1].mData?.assumingMemoryBound(to:Float.self) else {
                for buffer in output {
                    if let data = buffer.mData { memset(data, 0, Int(buffer.mDataByteSize)) }
                }
                return noErr
            }
            rm_audio_render(context.handle, left, right, Int(frameCount), 48000)
            return noErr
        }
        engine.attach(node)
        engine.connect(node, to:engine.mainMixerNode, format:format)
        source = node
        engine.mainMixerNode.outputVolume = muted ? 0 : 1
        do { try engine.start() } catch { stop(); throw error }
        let session = AVAudioSession.sharedInstance()
        NSLog("Removent audio output: rate=%g buffer_ms=%g latency_ms=%g",
            session.sampleRate, session.ioBufferDuration * 1000, session.outputLatency * 1000)
    }

    func stop() {
        epoch += 1
        engine.stop()
        if let source { engine.detach(source) }
        source = nil
        sessionQueue.async {
            try? AVAudioSession.sharedInstance().setActive(false, options:.notifyOthersOnDeactivation)
        }
    }
}
