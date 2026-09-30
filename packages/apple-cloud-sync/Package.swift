// swift-tools-version: 5.9
import PackageDescription

let package = Package(name: "RemoventCloudSync", platforms: [.macOS("26.0"), .iOS("26.0")],
    products: [.library(name: "RemoventCloudSync", targets: ["RemoventCloudSync"])],
    targets: [
        .target(name: "RemoventCloudSync", path: "Sources"),
        .testTarget(name: "RemoventCloudSyncTests", dependencies: ["RemoventCloudSync"], path: "Tests")
    ])
