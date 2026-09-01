// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "RemoventTray",
    defaultLocalization: "en",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "RemoventTray",
            path: "Sources/RemoventTray",
            resources: [.process("Resources")]
        )
    ]
)
