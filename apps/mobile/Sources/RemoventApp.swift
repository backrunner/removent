import SwiftUI

@main
struct RemoventApp: App {
    @UIApplicationDelegateAdaptor(MobileAppDelegate.self) private var delegate
    @StateObject private var model = ControllerModel()
    @Environment(\.scenePhase) private var phase
    var body: some Scene {
        WindowGroup {
            HomeView(model:model, discovery:model.discovery)
                .onAppear { delegate.sync = model.cloudSync }
                .onChange(of:phase) { _, phase in
                    if phase == .background { model.background() }
                    else if phase == .active { model.foreground() }
                }
        }
    }
}

struct HomeView: View {
    @ObservedObject var model: ControllerModel
    @ObservedObject var discovery: BonjourDiscovery
    @State private var draft: ConnectionDraft?
    @State private var settings = false
    @State private var pendingConnection: ConnectionDraft?
    var nearby: [NearbyDevice] {
        var seen = Set<String>()
        return (discovery.devices + model.scanDevices).filter { seen.insert($0.endpointKey).inserted }
    }
    var body: some View {
        NavigationStack {
            List {
                if let error = model.error, !model.presentingSession {
                    Section {
                        Label(error, systemImage: "exclamationmark.circle").foregroundStyle(.red).textSelection(.enabled)
                        Button(L("Dismiss", "关闭提示")) { model.error = nil }
                    }
                }
                Section(L("Saved computers", "已保存的电脑")) {
                    if model.bookmarks.isEmpty {
                        ContentUnavailableView(L("Your computers", "你的电脑"), systemImage:"desktopcomputer",
                            description:Text(L("Add a connection or choose a nearby computer.", "添加连接，或选择附近的电脑。")))
                            .listRowSeparator(.hidden)
                        Button { draft = ConnectionDraft() } label: {
                            Label(L("Add connection", "添加连接"), systemImage: "plus").frame(maxWidth: .infinity)
                        }.buttonStyle(PrimaryActionStyle()).padding(.bottom, 8).listRowSeparator(.hidden)
                    }
                    ForEach(model.bookmarks) { bookmark in
                        Button {
                            if bookmark.credentials_review_required { draft = ConnectionDraft(bookmark) }
                            else { model.connect(bookmark) }
                        } label: {
                            ComputerRow(name:bookmark.title, detail:bookmark.relay == nil ? "\(bookmark.host):\(bookmark.port)" : bookmark.host, protocolName:bookmark.protocol.title)
                        }
                        .buttonStyle(.plain)
                        .contextMenu {
                            Button(L("Edit", "编辑"), systemImage:"pencil") { draft = ConnectionDraft(bookmark) }
                            Button(L("Delete", "删除"), systemImage:"trash", role:.destructive) { model.delete(bookmark) }
                        }
                        .swipeActions {
                            Button(L("Delete", "删除"), role:.destructive) { model.delete(bookmark) }
                            Button(L("Edit", "编辑")) { draft = ConnectionDraft(bookmark) }.tint(.blue)
                        }
                    }
                }
                Section(L("Nearby", "附近")) {
                    ForEach(nearby) { device in
                        Button { draft = ConnectionDraft(device) } label: {
                            ComputerRow(name:device.name, detail:"\(device.host):\(device.port)", protocolName:device.protocol.title)
                        }
                        .buttonStyle(.plain)
                    }
                    if nearby.isEmpty {
                        Label(L("No computers found", "未发现电脑"), systemImage:"network").foregroundStyle(.secondary)
                    }
                    if let error = discovery.error { Text(error).font(.footnote).foregroundStyle(.secondary) }
                }
            }
            .listStyle(.insetGrouped)
            .frame(maxWidth: 900)
            .frame(maxWidth: .infinity)
            .background(Color(uiColor: .systemGroupedBackground))
            .navigationTitle("Removent")
            .toolbar {
                ToolbarItem(placement:.topBarLeading) {
                    Button(L("Settings", "设置"), systemImage:"gearshape") { settings = true }
                }
                ToolbarItem(placement:.topBarTrailing) {
                    Button(L("Add connection", "添加连接"), systemImage:"plus") { draft = ConnectionDraft() }
                        .accessibilityIdentifier("addConnection")
                }
            }
            .sheet(item:$draft, onDismiss: connectAfterDismissal) { draft in
                ConnectionForm(model:model, draft:draft) { pendingConnection = $0 }
            }
            .sheet(isPresented:$settings) { SettingsView(model:model) }
            .fullScreenCover(isPresented:$model.presentingSession) { SessionView(model:model) }
        }
    }
    private func connectAfterDismissal() {
        guard let connection = pendingConnection else { return }
        pendingConnection = nil
        do { try model.connect(connection) } catch { model.error = error.localizedDescription }
    }
}

struct ComputerRow: View {
    let name: String
    let detail: String
    let protocolName: String
    @Environment(\.dynamicTypeSize) private var typeSize
    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: "desktopcomputer")
                .font(.title2).foregroundStyle(.tint)
                .frame(width: 48, height: 48)
                .background(Color.accentColor.opacity(0.09), in: RoundedRectangle(cornerRadius: 14))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 5) {
                Text(name).foregroundStyle(.primary).font(.headline)
                Text(detail).font(.subheadline).foregroundStyle(.secondary)
                    .lineLimit(typeSize.isAccessibilitySize ? nil : 1).truncationMode(.middle)
                if typeSize.isAccessibilitySize { protocolBadge }
            }
            Spacer(minLength: 8)
            if !typeSize.isAccessibilitySize { protocolBadge }
            Image(systemName: "chevron.right").font(.caption.weight(.semibold))
                .foregroundStyle(.tertiary).accessibilityHidden(true)
        }
        .padding(.vertical, 8)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
    private var protocolBadge: some View {
        Text(protocolName).font(.caption.weight(.medium)).foregroundStyle(.secondary)
            .padding(.horizontal, 8).padding(.vertical, 4)
            .background(Color(uiColor: .tertiarySystemFill), in: Capsule())
    }
}

struct ConnectionForm: View {
    @ObservedObject var model: ControllerModel
    @State var draft: ConnectionDraft
    var onConnect: (ConnectionDraft) -> Void
    @Environment(\.dismiss) private var dismiss
    @Environment(\.dynamicTypeSize) private var typeSize
    @State private var failure: String?
    @State private var saveConnection = true
    @State private var connecting = false
    private enum Field: Hashable { case name, host, port, relayEndpoint, relayFingerprint, hostFingerprint, username, password, domain }
    @FocusState private var focused: Field?

    private var hasHost: Bool { !draft.host.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    var body: some View {
        NavigationStack {
            Form {
                Section(L("Computer", "电脑")) {
                    if typeSize.isAccessibilitySize {
                        protocolPicker.pickerStyle(.menu)
                    } else {
                        protocolPicker.pickerStyle(.segmented)
                    }
                    TextField(L("Name (optional)", "名称（选填）"), text: $draft.name)
                        .accessibilityIdentifier("connectionName").focused($focused, equals: .name)
                    VStack(alignment: .leading, spacing: 8) {
                        Text(draft.useRelay ? L("Room", "房间") : L("Address", "地址"))
                            .font(.subheadline).foregroundStyle(.secondary)
                        TextField(draft.useRelay ? L("Room name", "房间名称") : L("Hostname or IP address", "主机名或 IP 地址"), text: $draft.host)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                            .accessibilityIdentifier("connectionHost").focused($focused, equals: .host)
                    }.padding(.vertical, 4)
                    if !draft.useRelay {
                        LabeledContent(L("Port", "端口")) {
                            TextField(L("Port", "端口"), text: $draft.port)
                                .keyboardType(.numberPad).multilineTextAlignment(.trailing)
                                .accessibilityIdentifier("connectionPort").focused($focused, equals: .port)
                        }
                    }
                }
                if draft.protocol == .removent {
                    Section(L("Relay", "中继")) {
                        Toggle(L("Connect through relay", "通过中继连接"), isOn: $draft.useRelay)
                        if draft.useRelay {
                            TextField("removent://relay.example:443", text: $draft.relayEndpoint)
                                .focused($focused, equals: .relayEndpoint)
                                .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                            Picker(L("Transport", "传输方式"), selection: $draft.relayTransport) {
                                Text("Cloudflare / HTTPS").tag("websocket")
                                Text("VPS / QUIC").tag("quic")
                            }
                            if draft.relayTransport == "quic" {
                                TextField(L("Relay certificate fingerprint", "中继证书指纹"), text: $draft.relayFingerprint)
                                    .focused($focused, equals: .relayFingerprint)
                                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                            }
                            TextField(L("Host certificate fingerprint", "目标主机证书指纹"), text: $draft.hostFingerprint)
                                .focused($focused, equals: .hostFingerprint)
                                .textInputAutocapitalization(.never).autocorrectionDisabled()
                        }
                    }
                }
                if draft.protocol != .removent || draft.useRelay {
                    Section {
                        if draft.protocol != .removent {
                            TextField(draft.protocol == .vnc ? L("Username (Apple Remote Desktop)", "用户名（Apple Remote Desktop）") : L("Username", "用户名"), text: $draft.username)
                                .focused($focused, equals: .username)
                                .textInputAutocapitalization(.never).autocorrectionDisabled().textContentType(.username)
                        }
                        if draft.hasSavedPassword {
                            Toggle(L("Change saved password", "修改已保存的密码"), isOn: $draft.replaceSavedPassword)
                                .accessibilityIdentifier("replaceSavedPassword")
                        }
                        if !draft.hasSavedPassword || draft.replaceSavedPassword {
                            SecureField(draft.useRelay ? L("Controller token", "主控端凭据") : L("Password", "密码"), text: $draft.password)
                                .focused($focused, equals: .password)
                                .textContentType(.password).accessibilityIdentifier("connectionPassword")
                        }
                        if draft.protocol == .rdp {
                            TextField(L("Domain (optional)", "域（选填）"), text: $draft.domain)
                                .focused($focused, equals: .domain)
                                .textInputAutocapitalization(.never).autocorrectionDisabled()
                            Toggle(L("Allow untrusted certificate", "允许不受信任的证书"), isOn: $draft.allowUntrustedCertificate)
                                .accessibilityIdentifier("allowUntrustedCertificate")
                        }
                    } header: { Text(L("Authentication", "身份验证")) } footer: {
                        Text(draft.hasSavedPassword && !draft.replaceSavedPassword
                            ? L("The saved password is kept for this computer and account.", "将保留此电脑和账号已保存的密码。")
                            : L("Saved credentials are stored in Keychain.", "保存的凭据存放在系统钥匙串中。"))
                    }
                }
                Section {
                    Toggle(L("Save connection", "保存连接"), isOn: $saveConnection)
                        .accessibilityIdentifier("saveConnection")
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                VStack(spacing: 12) {
                    if let failure {
                        Label(failure, systemImage: "exclamationmark.circle")
                            .font(.footnote).foregroundStyle(.red).textSelection(.enabled)
                            .accessibilityIdentifier("connectionError")
                    }
                    Button(action: connect) {
                        Label(L("Connect", "连接"), systemImage: "arrow.up.right")
                            .frame(maxWidth: .infinity).frame(minHeight: 32)
                    }
                    .buttonStyle(PrimaryActionStyle()).controlSize(.large)
                    .disabled(connecting || !hasHost).accessibilityIdentifier("connectButton")
                }
                .padding().frame(maxWidth: 600).frame(maxWidth: .infinity)
                .background(.bar)
            }
            .navigationTitle(typeSize.isAccessibilitySize ? L("Connection", "连接") :
                draft.bookmarkID == nil ? L("Add connection", "添加连接") : L("Edit connection", "编辑连接"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(L("Cancel", "取消"), systemImage: "xmark") { dismiss() }.labelStyle(.iconOnly)
                }
                ToolbarItem(placement: .keyboard) {
                    Button(L("Done", "完成")) { focused = nil }.accessibilityIdentifier("dismissKeyboard")
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(L("Save", "保存"), systemImage: "checkmark") {
                        do { try model.save(draft); dismiss() } catch { failure = error.localizedDescription }
                    }.labelStyle(.iconOnly).disabled(!hasHost || connecting)
                }
            }
            .onChange(of: draft.protocol) { _, proto in
                draft.port = String(proto.port); draft.useRelay = false; draft.password = ""
                draft.username = ""; draft.domain = ""; draft.allowUntrustedCertificate = false
            }
            .onChange(of: draft.replaceSavedPassword) { _, _ in draft.password = "" }
        }
        .interactiveDismissDisabled(connecting)
    }
    private var protocolPicker: some View {
        Picker(L("Protocol", "协议"), selection: $draft.protocol) {
            ForEach(ConnectionProtocol.allCases) { Text($0.title).tag($0) }
        }.disabled(draft.bookmarkID != nil)
    }
    private func connect() {
        do {
            try model.validate(draft)
            if saveConnection { try model.save(draft) }
            connecting = true; focused = nil
            onConnect(draft)
            dismiss()
        } catch { failure = error.localizedDescription }
    }
}

struct SettingsView: View {
    @ObservedObject var model: ControllerModel
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Toggle(L("Remote audio", "远程音频"), isOn: $model.audioEnabled)
                    Toggle(L("Text clipboard", "文本剪贴板"), isOn: $model.clipboardEnabled)
                } header: { Text(L("Session", "会话")) } footer: {
                    Text(L("Applies to the next connection.", "将在下次连接时生效。"))
                }
                Section(L("LAN discovery", "局域网发现")) {
                    Toggle("Removent", isOn: $model.discoverRemovent)
                    Toggle("VNC", isOn: $model.discoverVNC)
                    Toggle("RDP", isOn: $model.discoverRDP)
                }
                if let sync = model.cloudSync { MobileCloudSyncSection(sync: sync) }
                Section(L("This device", "本机")) {
                    LabeledContent(L("Name", "名称"), value: UIDevice.current.name)
                    VStack(alignment: .leading, spacing: 8) {
                        Text(L("Certificate fingerprint", "证书指纹")).font(.subheadline).foregroundStyle(.secondary)
                        Text(model.fingerprint).font(.footnote.monospaced()).textSelection(.enabled)
                    }.padding(.vertical, 4)
                }
            }
            .navigationTitle(L("Settings", "设置"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) {
                Button(L("Done", "完成"), systemImage: "checkmark") { dismiss() }.labelStyle(.iconOnly)
            } }
        }
    }
}

/// System styles inherit Liquid Glass and the user's accessibility preferences.
struct PrimaryActionStyle: PrimitiveButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Button(configuration).buttonStyle(.glassProminent)
    }
}
