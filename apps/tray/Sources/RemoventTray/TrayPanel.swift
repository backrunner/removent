import SwiftUI

func trayText(_ key: String) -> String {
    NSLocalizedString(key, bundle: .trayResources, comment: "Tray panel")
}

struct TrayPanel: View {
    @ObservedObject var controller: StatusBarController

    private var status: StatusResponse? { controller.status }
    private var sharing: Bool { status?.running == true }
    private var permissionsMissing: Bool {
        guard let status else { return false }
        return !status.screen_recording_granted || !status.accessibility_granted
    }
    private var stateKey: String {
        if !controller.connected { return controller.stoppedByUser ? "status.process_stopped" : "status.daemon_offline" }
        if !sharing { return "status.stopped" }
        if permissionsMissing { return "panel.permissions_needed" }
        return status?.host_ready == true ? "status.running" : "status.starting"
    }
    private var stateColor: Color {
        sharing && !permissionsMissing && status?.host_ready == true ? .green : .secondary
    }

    var body: some View {
        TrayPopoverContent(width: controller.panelWidth, maximumHeight: controller.panelMaximumHeight) {
            panelContent
        }
    }

    private var panelContent: some View {
        VStack(alignment: .leading, spacing: 16) {
            header
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Text(trayText("panel.share_mac")).fontWeight(.medium)
                    Spacer()
                    Toggle(trayText("panel.share_mac"), isOn: Binding(
                        get: { sharing }, set: { _ in controller.toggleService() }))
                        .labelsHidden().toggleStyle(.switch).disabled(controller.serviceBusy)
                }
                Text(trayText("panel.share_detail"))
                    .font(.caption).foregroundStyle(.secondary)
                if let error = controller.recoveryError ?? status?.host_error {
                    Text(error).font(.caption).foregroundStyle(.orange)
                        .lineLimit(3).help(error).textSelection(.enabled)
                }
            }
            Divider()
            if status == nil || permissionsMissing {
                permissionSection
                Divider()
            }
            sessionsSection
            if let pin = controller.pendingPin, !pin.isEmpty {
                HStack {
                    Text(trayText("panel.pairing_code")).foregroundStyle(.secondary)
                    Spacer()
                    Text(pin).font(.system(.title3, design: .monospaced).weight(.semibold))
                        .textSelection(.enabled)
                }
            }
            Button(action: controller.openMainApp) {
                Label(trayText("menu.open_main_window"), systemImage: "macwindow")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent).controlSize(.large)
            .keyboardShortcut("o", modifiers: .command)
            footer
        }
        .padding(18)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(nsImage: BrandIcon.app).resizable().frame(width: 40, height: 40)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text("Removent").font(.headline)
                Text(status?.device_name ?? Host.current().localizedName ?? "Mac")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            HStack(spacing: 5) {
                Circle().fill(permissionsMissing && sharing ? .orange : stateColor)
                    .frame(width: 6, height: 6)
                Text(trayText(stateKey)).font(.caption)
            }
            .accessibilityElement(children: .combine)
        }
    }

    private var permissionSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(trayText("panel.permissions")).font(.subheadline.weight(.semibold))
            permissionRow(.screenRecording, symbol: "record.circle",
                          granted: status.map(\.screen_recording_granted))
            permissionRow(.accessibility, symbol: "accessibility",
                          granted: status.map(\.accessibility_granted))
            if permissionsMissing {
                Text(trayText("panel.permission_detail"))
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func permissionRow(_ permission: HostPermission, symbol: String, granted: Bool?) -> some View {
        HStack(spacing: 9) {
            Image(systemName: symbol).foregroundStyle(.secondary).frame(width: 18)
            Text(trayText("panel.\(permission.rawValue)"))
            Spacer()
            if granted == true {
                Label(trayText("panel.allowed"), systemImage: "checkmark.circle.fill")
                    .font(.caption).foregroundStyle(.green)
            } else if granted == nil {
                Text(trayText("panel.unavailable")).font(.caption).foregroundStyle(.secondary)
            } else {
                Button(trayText("panel.request")) { controller.requestPermission(permission) }
                    .controlSize(.small)
                    .accessibilityLabel(trayText("panel.request") + trayText("panel.\(permission.rawValue)"))
                Menu {
                    Button(trayText("panel.system_settings")) { controller.openPermissionSettings(permission) }
                    Button(trayText("menu.restart_service"), action: controller.restartService)
                        .disabled(controller.serviceBusy || !(status?.sessions.isEmpty ?? true))
                } label: {
                    Image(systemName: "ellipsis").frame(width: 14, height: 14)
                }
                .menuStyle(.borderlessButton)
                .controlSize(.small).menuIndicator(.hidden).fixedSize()
                .accessibilityLabel(trayText("panel.\(permission.rawValue)") + trayText("panel.options"))
            }
        }
    }

    private var sessionsSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(trayText("panel.connections")).font(.subheadline.weight(.semibold))
                Spacer()
                Text("\(status?.sessions.count ?? 0)").foregroundStyle(.secondary).monospacedDigit()
            }
            if let sessions = status?.sessions, !sessions.isEmpty {
                ScrollView {
                    VStack(spacing: 10) {
                        ForEach(sessions, id: \.id) { session in
                            HStack(spacing: 8) {
                                Image(systemName: "desktopcomputer").foregroundStyle(.secondary)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(session.peer_name).lineLimit(1)
                                    Text(controller.durationString(since: session.since_unix))
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                Button(trayText("panel.disconnect")) { controller.client.kickSession(session.id) }
                                    .controlSize(.small)
                            }
                        }
                    }
                }.frame(height: CGFloat(min(sessions.count, 3)) * 42)
            } else {
                Text(trayText("menu.no_sessions")).font(.caption).foregroundStyle(.secondary)
            }
            if let relay = status?.relay_connected {
                Label(trayText(relay ? "menu.relay_online" : "menu.relay_offline"), systemImage: "network")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var footer: some View {
        HStack {
            Text(buildLabel).font(.caption2).foregroundStyle(.tertiary)
                .help(controller.outerBundle.path)
            Spacer()
            TrayOptionsMenu(controller: controller)
        }
    }

    private var buildLabel: String {
        let bundle = Bundle(url: controller.outerBundle) ?? .main
        let version = bundle.object(forInfoDictionaryKey: "RemoventReleaseVersion") as? String
            ?? bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "Dev"
        let revision = bundle.object(forInfoDictionaryKey: "RemoventBuildRevision") as? String
        return revision.map { "\(version) · \($0)" } ?? version
    }
}
