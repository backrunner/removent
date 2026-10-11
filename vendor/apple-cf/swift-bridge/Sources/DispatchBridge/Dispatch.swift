// Dispatch Bridge - DispatchQueue

import Foundation

// MARK: - Dispatch Queue Management

func acfDispatchQoS(_ qos: Int32) -> DispatchQoS {
    switch qos {
    case 0: .background
    case 1: .utility
    case 2: .default
    case 3: .userInitiated
    case 4: .userInteractive
    default: .default
    }
}

@_cdecl("acf_dispatch_queue_create")
public func createDispatchQueue(_ label: UnsafePointer<CChar>, _ qos: Int32) -> UnsafeMutableRawPointer {
    let labelStr = String(cString: label)
    let queue = DispatchQueue(label: labelStr, qos: acfDispatchQoS(qos))
    return Unmanaged.passRetained(queue).toOpaque()
}

@_cdecl("acf_dispatch_queue_create_concurrent")
public func acf_dispatch_queue_create_concurrent(_ label: UnsafePointer<CChar>, _ qos: Int32) -> UnsafeMutableRawPointer {
    let queue = DispatchQueue(label: String(cString: label), qos: acfDispatchQoS(qos), attributes: .concurrent)
    return Unmanaged.passRetained(queue).toOpaque()
}

@_cdecl("acf_dispatch_queue_main")
public func acf_dispatch_queue_main() -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(DispatchQueue.main).toOpaque()
}

@_cdecl("acf_dispatch_queue_global")
public func acf_dispatch_queue_global(_ qos: Int32) -> UnsafeMutableRawPointer {
    Unmanaged.passRetained(DispatchQueue.global(qos: acfDispatchQoS(qos).qosClass)).toOpaque()
}

@_cdecl("dispatch_queue_release")
public func releaseDispatchQueue(_ queue: UnsafeMutableRawPointer) {
    Unmanaged<DispatchQueue>.fromOpaque(queue).release()
}

@_cdecl("dispatch_queue_retain")
public func retainDispatchQueue(_ queue: UnsafeMutableRawPointer) -> UnsafeMutableRawPointer {
    let q = Unmanaged<DispatchQueue>.fromOpaque(queue).takeUnretainedValue()
    return Unmanaged.passRetained(q).toOpaque()
}
