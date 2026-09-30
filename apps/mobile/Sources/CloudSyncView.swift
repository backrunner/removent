import SwiftUI
import UIKit

@MainActor
final class MobileAppDelegate: NSObject, UIApplicationDelegate {
    weak var sync: ConnectionSyncService?
    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        application.registerForRemoteNotifications(); return true
    }
    func application(_ application: UIApplication, didReceiveRemoteNotification userInfo: [AnyHashable: Any], fetchCompletionHandler completionHandler: @escaping (UIBackgroundFetchResult) -> Void) {
        Task {
            guard let sync else { completionHandler(.noData); return }
            let previous = sync.status.last_success
            await sync.refresh(force: true)
            completionHandler(sync.status.last_success > previous ? .newData : .noData)
        }
    }
}

struct MobileCloudSyncSection: View {
    @ObservedObject var sync: ConnectionSyncService
    private func describe(_ connection: CloudSyncConnection?) -> String {
        connection?.summary(relayLabel: L("Relay", "中继"), relayIDLabel: L("Relay fingerprint", "中继指纹"),
                            hostIDLabel: L("Host fingerprint", "主机指纹")) ?? L("Deleted", "已删除")
    }
    private var statusText: String {
        switch sync.status.code {
        case "off": return L("Off", "已关闭")
        case "waiting": return L("Waiting to sync", "等待同步")
        case "syncing": return L("Syncing…", "正在同步…")
        case "ready": return sync.status.pending > 0 ? L("Changes waiting to sync", "有更改待同步") : L("Up to date", "已同步")
        case "offline": return L("Waiting for a connection", "等待网络连接")
        case "quota": return L("iCloud storage is full", "iCloud 储存空间已满")
        case "account_unavailable": return L("Sign in to iCloud in Settings", "请在系统设置中登录 iCloud")
        case "configuration": return L("iCloud is unavailable in this build", "当前构建尚未配置 iCloud")
        case "upgrade_required": return L("Update Removent to continue syncing", "请更新 Removent 以继续同步")
        case "conflict": return L("Choose which changes to keep", "请选择要保留的更改")
        default: return L("Could not sync. Your local changes are saved.", "暂时无法同步，本机更改已保存。")
        }
    }
    var body: some View {
        Section {
            Toggle(L("Sync saved connections", "同步已保存的连接"), isOn: Binding(get: { sync.status.enabled }, set: sync.setEnabled))
                .accessibilityIdentifier("cloudSyncToggle")
            LabeledContent(L("Status", "状态"), value: statusText).accessibilityIdentifier("cloudSyncStatus")
            if sync.status.enabled {
                if sync.status.last_success > 0 {
                    LabeledContent(L("Last synced", "上次同步")) {
                        Text(Date(timeIntervalSince1970: sync.status.last_success), format: .dateTime.month().day().hour().minute())
                    }
                }
                if sync.status.pending > 0 {
                    LabeledContent(L("Pending changes", "待同步更改"), value: String(sync.status.pending))
                }
                Button(L("Sync now", "立即同步"), systemImage: "arrow.triangle.2.circlepath") { sync.retry() }
                    .disabled(sync.status.code == "syncing")
                ForEach(sync.status.conflicts) { conflict in
                    VStack(alignment: .leading, spacing: 12) {
                        Text(L("Conflicting changes", "更改冲突")).font(.headline)
                        Text(L("This device: ", "本机：") + describe(conflict.local))
                        Text(L("iCloud: ", "iCloud：") + describe(conflict.remote))
                        Button(L("Keep this device's version", "保留本机版本")) { sync.resolve(conflict.id, keepLocal: true) }
                        Button(L("Use iCloud version", "使用 iCloud 版本")) { sync.resolve(conflict.id, keepLocal: false) }
                    }.font(.subheadline).padding(.vertical, 6)
                }
            }
            if let error = sync.localError { Text(error).foregroundStyle(.red).font(.footnote) }
        } header: { Text("iCloud") } footer: {
            Text(L("Share saved connections with Removent on your other devices using the same iCloud account. Passwords and device pairing stay on each device. Turning sync off keeps local connections.",
                   "通过同一 iCloud 账户，与其他设备上的 Removent 共享已保存的连接。密码和设备配对保留在各自设备上。关闭同步会保留本机连接。"))
        }
    }
}
