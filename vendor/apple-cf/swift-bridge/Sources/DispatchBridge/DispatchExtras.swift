import Foundation

@_cdecl("acf_dispatch_async_f")
public func acf_dispatch_async_f(
    _ queue: UnsafeMutableRawPointer,
    _ context: UnsafeMutableRawPointer?,
    _ work: (@convention(c) (UnsafeMutableRawPointer?) -> Void)?
) {
    guard let work else { return }
    let queue = Unmanaged<DispatchQueue>.fromOpaque(queue).takeUnretainedValue()
    queue.async {
        work(context)
    }
}

@_cdecl("acf_dispatch_async_and_wait_f")
public func acf_dispatch_async_and_wait_f(
    _ queue: UnsafeMutableRawPointer,
    _ context: UnsafeMutableRawPointer?,
    _ work: (@convention(c) (UnsafeMutableRawPointer?) -> Void)?
) {
    guard let work else { return }
    let queue = Unmanaged<DispatchQueue>.fromOpaque(queue).takeUnretainedValue()
    queue.asyncAndWait {
        work(context)
    }
}

@_cdecl("acf_dispatch_after_f")
public func acf_dispatch_after_f(
    _ delayNs: UInt64,
    _ queue: UnsafeMutableRawPointer,
    _ context: UnsafeMutableRawPointer?,
    _ work: (@convention(c) (UnsafeMutableRawPointer?) -> Void)?
) {
    guard let work else { return }
    let queue = Unmanaged<DispatchQueue>.fromOpaque(queue).takeUnretainedValue()
    queue.asyncAfter(deadline: .now() + .nanoseconds(Int(clamping: delayNs))) {
        work(context)
    }
}

@_cdecl("acf_dispatch_apply_f")
public func acf_dispatch_apply_f(
    _ iterations: Int,
    _ queue: UnsafeMutableRawPointer,
    _ context: UnsafeMutableRawPointer?,
    _ work: (@convention(c) (Int, UnsafeMutableRawPointer?) -> Void)?
) {
    guard let work, iterations > 0 else { return }
    let queue = Unmanaged<DispatchQueue>.fromOpaque(queue).takeUnretainedValue()
    let group = DispatchGroup()
    for iteration in 0..<iterations {
        group.enter()
        queue.async {
            work(iteration, context)
            group.leave()
        }
    }
    group.wait()
}

private func dispatchDeadline(timeoutMs: Int64) -> DispatchTime {
    if timeoutMs < 0 {
        return .distantFuture
    }
    return .now() + .milliseconds(Int(timeoutMs))
}

final class DispatchSourceTimerHolder {
    private enum State {
        case inactive
        case active
        case cancelled
    }

    let source: DispatchSourceTimer
    private let lock = NSLock()
    private var state = State.inactive
    private var fireCount: UInt64 = 0

    init(intervalNs: UInt64, leewayNs: UInt64) {
        let queue = DispatchQueue(label: "com.doomfish.apple-cf.dispatch-source")
        source = DispatchSource.makeTimerSource(queue: queue)
        source.setEventHandler { [weak self] in
            self?.recordFire()
        }
        let interval = DispatchTimeInterval.nanoseconds(Int(clamping: intervalNs))
        source.schedule(
            deadline: .now() + interval,
            repeating: interval,
            leeway: .nanoseconds(Int(clamping: leewayNs))
        )
    }

    deinit {
        cancel()
    }

    func resume() {
        lock.lock()
        guard state == .inactive else {
            lock.unlock()
            return
        }
        state = .active
        lock.unlock()
        source.activate()
    }

    func cancel() {
        lock.lock()
        let activateAfterCancel = state == .inactive
        guard state != .cancelled else {
            lock.unlock()
            return
        }
        state = .cancelled
        lock.unlock()

        source.cancel()
        if activateAfterCancel {
            source.activate()
        }
    }

    func currentFireCount() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return fireCount
    }

    private func recordFire() {
        let delta = UInt64(source.data)
        lock.lock()
        let (newValue, overflow) = fireCount.addingReportingOverflow(delta)
        fireCount = overflow ? UInt64.max : newValue
        lock.unlock()
    }
}

final class DispatchGroupHolder {
    let group = DispatchGroup()
    private let lock = NSLock()
    private var entered = 0

    func enter() {
        lock.lock()
        defer { lock.unlock() }
        entered += 1
        group.enter()
    }

    func leave() {
        lock.lock()
        defer { lock.unlock() }
        guard entered > 0 else { return }
        entered -= 1
        group.leave()
    }

    deinit {
        for _ in 0..<entered {
            group.leave()
        }
    }
}

final class DispatchSemaphoreHolder {
    let semaphore: DispatchSemaphore
    private let lock = NSLock()
    private var consumed = 0

    init(value: Int) {
        semaphore = DispatchSemaphore(value: value)
    }

    func signal() -> Int {
        lock.lock()
        defer { lock.unlock() }
        consumed &-= 1
        return semaphore.signal()
    }

    func wait(timeout: DispatchTime) -> Bool {
        guard semaphore.wait(timeout: timeout) == .success else { return false }
        lock.lock()
        consumed &+= 1
        lock.unlock()
        return true
    }

    deinit {
        for _ in 0..<max(consumed, 0) {
            semaphore.signal()
        }
    }
}

@_cdecl("acf_dispatch_group_holder_create")
public func acf_dispatch_group_holder_create() -> UnsafeMutableRawPointer {
    return Unmanaged.passRetained(DispatchGroupHolder()).toOpaque()
}

@_cdecl("acf_dispatch_group_holder_enter")
public func acf_dispatch_group_holder_enter(_ group: UnsafeMutableRawPointer) {
    let group = Unmanaged<DispatchGroupHolder>.fromOpaque(group).takeUnretainedValue()
    group.enter()
}

@_cdecl("acf_dispatch_group_holder_leave")
public func acf_dispatch_group_holder_leave(_ group: UnsafeMutableRawPointer) {
    let group = Unmanaged<DispatchGroupHolder>.fromOpaque(group).takeUnretainedValue()
    group.leave()
}

@_cdecl("acf_dispatch_group_holder_wait")
public func acf_dispatch_group_holder_wait(_ group: UnsafeMutableRawPointer, _ timeoutMs: Int64) -> Bool {
    let group = Unmanaged<DispatchGroupHolder>.fromOpaque(group).takeUnretainedValue()
    return group.group.wait(timeout: dispatchDeadline(timeoutMs: timeoutMs)) == .success
}

@_cdecl("acf_dispatch_semaphore_holder_create")
public func acf_dispatch_semaphore_holder_create(_ value: Int64) -> UnsafeMutableRawPointer? {
    guard value >= 0, let value = Int(exactly: value) else { return nil }
    return Unmanaged.passRetained(DispatchSemaphoreHolder(value: value)).toOpaque()
}

@_cdecl("acf_dispatch_semaphore_holder_signal")
public func acf_dispatch_semaphore_holder_signal(_ semaphore: UnsafeMutableRawPointer) -> Int64 {
    let semaphore = Unmanaged<DispatchSemaphoreHolder>.fromOpaque(semaphore).takeUnretainedValue()
    return Int64(semaphore.signal())
}

@_cdecl("acf_dispatch_semaphore_holder_wait")
public func acf_dispatch_semaphore_holder_wait(_ semaphore: UnsafeMutableRawPointer, _ timeoutMs: Int64) -> Bool {
    let semaphore = Unmanaged<DispatchSemaphoreHolder>.fromOpaque(semaphore).takeUnretainedValue()
    return semaphore.wait(timeout: dispatchDeadline(timeoutMs: timeoutMs))
}

@_cdecl("acf_dispatch_source_timer_create")
public func acf_dispatch_source_timer_create(_ intervalMs: UInt64, _ leewayMs: UInt64) -> UnsafeMutableRawPointer {
    let (intervalNs, intervalOverflow) = intervalMs.multipliedReportingOverflow(by: 1_000_000)
    let (leewayNs, leewayOverflow) = leewayMs.multipliedReportingOverflow(by: 1_000_000)
    return acf_dispatch_source_timer_create_ns(
        intervalOverflow ? UInt64.max : intervalNs,
        leewayOverflow ? UInt64.max : leewayNs
    )
}

@_cdecl("acf_dispatch_source_timer_create_ns")
public func acf_dispatch_source_timer_create_ns(_ intervalNs: UInt64, _ leewayNs: UInt64) -> UnsafeMutableRawPointer {
    return Unmanaged.passRetained(DispatchSourceTimerHolder(intervalNs: intervalNs, leewayNs: leewayNs)).toOpaque()
}

@_cdecl("acf_dispatch_source_timer_resume")
public func acf_dispatch_source_timer_resume(_ source: UnsafeMutableRawPointer) {
    let source = Unmanaged<DispatchSourceTimerHolder>.fromOpaque(source).takeUnretainedValue()
    source.resume()
}

@_cdecl("acf_dispatch_source_timer_cancel")
public func acf_dispatch_source_timer_cancel(_ source: UnsafeMutableRawPointer) {
    let source = Unmanaged<DispatchSourceTimerHolder>.fromOpaque(source).takeUnretainedValue()
    source.cancel()
}

@_cdecl("acf_dispatch_source_timer_fire_count")
public func acf_dispatch_source_timer_fire_count(_ source: UnsafeMutableRawPointer) -> UInt64 {
    let source = Unmanaged<DispatchSourceTimerHolder>.fromOpaque(source).takeUnretainedValue()
    return source.currentFireCount()
}
