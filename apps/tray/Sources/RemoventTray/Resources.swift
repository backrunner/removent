import Foundation

extension Bundle {
    /// SwiftPM's generated accessor varies between toolchains and may only
    /// search beside the .app or on the build machine. Installed resources live
    /// in Contents/Resources; resolve them before touching Bundle.module.
    static let trayResources: Bundle = {
        for base in [Bundle.main.resourceURL, Bundle.main.bundleURL] {
            if let url = base?.appendingPathComponent("RemoventTray_RemoventTray.bundle"),
               let bundle = Bundle(url: url) { return bundle }
        }
        if Bundle.main.bundleURL.pathExtension == "app" {
            // A damaged install should still expose service controls.
            trayLog("localization bundle missing from installed tray")
            return Bundle.main
        }
        return .module
    }()
}
