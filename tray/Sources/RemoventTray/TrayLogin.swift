import Foundation

func canManageTrayLogin(bundleID: String?, dataDirectory: URL, home: URL) -> Bool {
    let installedData = home.appendingPathComponent("Library/Application Support/removent/userdata")
    return bundleID == "com.alkinum.removent.tray"
        && dataDirectory.standardizedFileURL.resolvingSymlinksInPath()
            == installedData.standardizedFileURL.resolvingSymlinksInPath()
}
