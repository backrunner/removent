import SwiftUI

struct TrayOptionsMenu: View {
    @ObservedObject var controller: StatusBarController
    @State private var hovered = false

    private var serviceActionDisabled: Bool {
        controller.serviceBusy || !(controller.status?.sessions.isEmpty ?? true) || controller.serviceCLI == nil
    }

    var body: some View {
        Menu {
            Section(trayText("menu.section.service")) {
                Button(action: controller.setupPermissions) {
                    Label(trayText("menu.setup_permissions"), systemImage: "lock.shield")
                }
                .disabled(!controller.connected)
                Button(action: controller.restartService) {
                    Label(trayText("menu.restart_service"), systemImage: "arrow.clockwise")
                }
                .disabled(serviceActionDisabled)
                Button(action: controller.toggleProcess) {
                    Label(trayText(controller.connected ? "menu.stop_process" : "menu.start_process"),
                          systemImage: controller.connected ? "stop.circle" : "play.circle")
                }
                .disabled(serviceActionDisabled)
            }
            Section(trayText("menu.section.login")) {
                Toggle(trayText("options.start_service"), isOn: Binding(
                    get: { controller.loginEnabled }, set: { _ in controller.toggleLaunchAtLogin() }))
                    .disabled(controller.serviceBusy || controller.serviceCLI == nil)
                Toggle(trayText("options.show_tray"), isOn: Binding(
                    get: { controller.ownsTrayLoginItem && FileManager.default.fileExists(atPath: controller.trayLoginURL.path) },
                    set: { _ in controller.toggleTrayAtLogin() }))
                    .disabled(!controller.ownsTrayLoginItem)
            }
            Section(trayText("menu.section.app")) {
                Button(action: controller.openDataDirectory) {
                    Label(trayText("menu.open_data_directory"), systemImage: "folder")
                }
                Button(action: controller.quitTray) {
                    Label(trayText("menu.quit"), systemImage: "rectangle.portrait.and.arrow.right")
                }
            }
        } label: {
            Text(trayText("panel.options")).font(.caption).foregroundStyle(.primary)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.visible)
        .menuOrder(.fixed).fixedSize()
        .padding(.horizontal, 9).padding(.vertical, 5)
        .background(.primary.opacity(hovered ? 0.12 : 0.05), in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(.primary.opacity(0.08)))
        .contentShape(RoundedRectangle(cornerRadius: 6))
        .onHover { hovered = $0 }
        .accessibilityLabel(trayText("panel.options"))
    }
}
