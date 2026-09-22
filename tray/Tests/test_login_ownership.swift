import Foundation

@main
struct LoginOwnershipTest {
    static func main() {
        let home = URL(fileURLWithPath: "/tmp/removent-login-ownership")
        let data = home.appendingPathComponent("Library/Application Support/removent/userdata")
        let bundle = "com.alkinum.removent.tray"
        precondition(canManageTrayLogin(bundleID: bundle, dataDirectory: data, home: home))
        precondition(!canManageTrayLogin(bundleID: nil, dataDirectory: data, home: home))
        precondition(!canManageTrayLogin(bundleID: "dev.tray", dataDirectory: data, home: home))
        precondition(!canManageTrayLogin(bundleID: bundle, dataDirectory: home.appendingPathComponent("custom"), home: home))
        print("PASS: tray login ownership isolates source builds and custom roots")
    }
}
