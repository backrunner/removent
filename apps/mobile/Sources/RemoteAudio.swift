import AVFoundation

@MainActor
final class RemoteAudio {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private var format: AVAudioFormat?
    private var queued = 0
    private var epoch = 0
    private let sessionQueue = DispatchQueue(label: "com.alkinum.removent.audio-session")
    var muted = false { didSet { player.volume = muted ? 0 : 1 } }

    init() { engine.attach(player) }
    func start(rate: Double, channels: Int) async throws {
        stop()
        let current = epoch
        guard (8000...96000).contains(rate), (1...2).contains(channels),
            let format = AVAudioFormat(standardFormatWithSampleRate:rate, channels:AVAudioChannelCount(channels)) else {
            throw MobileError.message("Unsupported audio format")
        }
        // Audio-session activation can block on the system audio service. Keep it
        // off the UI thread and serialize it with teardown during reconnects.
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            sessionQueue.async {
                do {
                    let session = AVAudioSession.sharedInstance()
                    try session.setCategory(.playback, mode:.default)
                    try session.setPreferredIOBufferDuration(0.01)
                    try session.setActive(true)
                    continuation.resume()
                } catch { continuation.resume(throwing: error) }
            }
        }
        guard current == epoch else { return }
        engine.connect(player, to:engine.mainMixerNode, format:format)
        self.format = format
        try engine.start(); player.volume = muted ? 0 : 1; player.play()
    }
    func push(_ pcm: RMAudio) {
        guard let format, engine.isRunning, pcm.channels == format.channelCount,
            Double(pcm.sample_rate) == format.sampleRate, pcm.len > 0,
            let source = pcm.data else { return }
        // At most 120 ms of scheduled audio; discard old playback after a UI stall.
        if queued >= 12 { player.stop(); epoch += 1; queued = 0; player.play() }
        let channels = Int(pcm.channels)
        let count = pcm.len / channels
        guard let buffer = AVAudioPCMBuffer(pcmFormat:format, frameCapacity:AVAudioFrameCount(count)),
            let output = buffer.floatChannelData else { return }
        buffer.frameLength = AVAudioFrameCount(count)
        for channel in 0..<channels {
            for sample in 0..<count { output[channel][sample] = Float(source[sample * channels + channel]) / 32768 }
        }
        queued += 1
        let current = epoch
        player.scheduleBuffer(buffer, completionCallbackType:.dataPlayedBack) { [weak self] _ in
            Task { @MainActor in
                if let self, self.epoch == current { self.queued = max(0, self.queued - 1) }
            }
        }
    }
    func stop() {
        epoch += 1; queued = 0; player.stop(); engine.stop(); format = nil
        sessionQueue.async {
            try? AVAudioSession.sharedInstance().setActive(false, options:.notifyOthersOnDeactivation)
        }
    }
}
