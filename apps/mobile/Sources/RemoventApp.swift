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
                if model.bookmarks.isEmpty {
                    Section {
                        welcome
                            .listRowInsets(EdgeInsets(top: 20, leading: 20, bottom: 20, trailing: 20))
                            .listRowSeparator(.hidden)
                    }
                }
                if !model.bookmarks.isEmpty {
                    Section(L("Saved computers", "已保存的电脑")) {
                        ForEach(model.bookmarks) { bookmark in
                            Button {
                                if bookmark.credentials_review_required { draft = ConnectionDraft(bookmark) }
                                else { model.connect(bookmark) }
                            } label: {
                                ComputerRow(name:bookmark.title, detail:bookmark.relay == nil ? "\(bookmark.host):\(bookmark.port)" : bookmark.host, protocolName:bookmark.protocol.title)
                            }
                            .accessibilityIdentifier("savedConnection-\(bookmark.id)")
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
                }
                Section {
                    ForEach(nearby) { device in
                        Button { draft = ConnectionDraft(device) } label: {
                            ComputerRow(name:device.name, detail:"\(device.host):\(device.port)", protocolName:device.protocol.title)
                        }
                        .buttonStyle(.plain)
                    }
                    if nearby.isEmpty {
                        Label(L("No computers found", "未发现电脑"), systemImage: "wifi")
                            .foregroundStyle(.secondary)
                            .padding(.vertical, 4)
                    }
                    if let error = discovery.error { Text(error).font(.footnote).foregroundStyle(.secondary) }
                } header: {
                    Text(L("Nearby computers", "附近的电脑"))
                } footer: {
                    if nearby.isEmpty {
                        Text(L("Keep your computer on the same network with screen sharing enabled.",
                               "让电脑连接同一网络，并开启屏幕共享。"))
                    }
                }
            }
            .listStyle(.insetGrouped)
            .listSectionSpacing(20)
            .contentMargins(.top, 12, for: .scrollContent)
            .modifier(MobileContentWidth())
            .navigationTitle("Removent")
            .navigationBarTitleDisplayMode(.inline)
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
    private var welcome: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 28, weight: .regular)).foregroundStyle(.tint)
                    .frame(width: 36, height: 36).accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 6) {
                    Text(L("Connect to your computer", "连接到你的电脑"))
                        .font(.headline).foregroundStyle(.primary)
                    Text(L("Add an address, or choose a computer nearby.", "添加电脑地址，或选择附近的电脑。"))
                        .font(.subheadline).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Button { draft = ConnectionDraft() } label: {
                Text(L("Add connection", "添加连接")).frame(maxWidth: .infinity)
            }
            .buttonStyle(PrimaryActionStyle()).controlSize(.large)
            .accessibilityIdentifier("emptyAddConnection")
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
        HStack(alignment: .center, spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.title3).foregroundStyle(.secondary)
                .frame(width: 28).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(name).foregroundStyle(.primary).font(.body.weight(.medium))
                    .lineLimit(typeSize.isAccessibilitySize ? nil : 1)
                Text("\(protocolName) · \(detail)")
                    .font(.subheadline).foregroundStyle(.secondary)
                    .lineLimit(typeSize.isAccessibilitySize ? nil : 1).truncationMode(.middle)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 4)
        .frame(minHeight: 44)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityHint(L("Connect to this computer", "连接到此电脑"))
    }
}

/// Keep phone content full width and use a readable column in wide iPad sheets.
struct MobileContentWidth: ViewModifier {
    func body(content: Content) -> some View {
        content.frame(maxWidth: 720).frame(maxWidth: .infinity)
            .background(Color(uiColor: .systemGroupedBackground))
    }
}

/// Labels, controls and text fields follow the same adaptive form alignment.
struct ConnectionField<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.subheadline).foregroundStyle(.secondary)
            content.frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 4)
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
    private enum Field: Hashable { case name, host, port, relayEndpoint, relayServerName, username, password, domain }
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
                    ConnectionField(title: L("Name", "名称")) {
                        TextField(L("Optional", "选填"), text: $draft.name)
                            .accessibilityIdentifier("connectionName").focused($focused, equals: .name)
                    }
                    ConnectionField(title: draft.useRelay ? L("Room or connection code", "房间或连接码") : (draft.protocol == .removent ? L("Address or connection code", "地址或连接码") : L("Address", "地址"))) {
                        TextField(draft.useRelay ? L("Room or code", "房间或连接码") : (draft.protocol == .removent ? L("IP, hostname or code", "IP、域名或连接码") : L("IP or hostname", "IP 或域名")), text: $draft.host)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                            .accessibilityIdentifier("connectionHost").focused($focused, equals: .host)
                    }
                    if !draft.useRelay && !draft.isPairingCode {
                        ConnectionField(title: L("Port", "端口")) {
                            TextField(L("Port", "端口"), text: $draft.port)
                                .keyboardType(.numberPad)
                                .accessibilityIdentifier("connectionPort").focused($focused, equals: .port)
                        }
                    }
                }
                if draft.protocol == .removent {
                    Section(L("Relay", "中继")) {
                        Toggle(L("Connect through relay", "通过中继连接"), isOn: $draft.useRelay)
                            .accessibilityIdentifier("useRelay")
                        if draft.useRelay {
                            ConnectionField(title: L("Relay address", "中继地址")) {
                                TextField("removent://relay.example:443", text: $draft.relayEndpoint)
                                    .focused($focused, equals: .relayEndpoint).accessibilityIdentifier("relayEndpoint")
                                    .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                            }
                            Picker(L("Transport", "传输方式"), selection: $draft.relayTransport) {
                                Text("Cloudflare / HTTPS").tag("websocket")
                                Text("VPS / QUIC").tag("quic")
                            }.accessibilityIdentifier("relayTransport")
                            ConnectionField(title: L("SNI (optional)", "SNI（选填）")) {
                                TextField(L("Default: relay hostname", "默认使用中继域名"), text: $draft.relayServerName)
                                    .focused($focused, equals: .relayServerName)
                                    .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.URL)
                                    .accessibilityIdentifier("relayServerName")
                            }
                            Toggle(L("Verify relay certificate", "验证中继证书"), isOn: $draft.verifyRelayCertificate)
                                .accessibilityIdentifier("verifyRelayCertificate")
                            if !draft.verifyRelayCertificate {
                                Text(L("The connection is encrypted, but the relay's identity is not verified.", "连接仍然加密，但不验证中继的身份。"))
                                    .font(.footnote).foregroundStyle(.secondary)
                            }

                        }
                    }
                }
                if draft.protocol != .removent || draft.useRelay {
                    Section {
                        if draft.protocol != .removent {
                            ConnectionField(title: L("Username", "用户名")) {
                                TextField(draft.protocol == .vnc ? L("Optional for VNC", "VNC 可选填") : L("Username", "用户名"), text: $draft.username)
                                    .focused($focused, equals: .username)
                                    .textInputAutocapitalization(.never).autocorrectionDisabled().textContentType(.username)
                            }
                        }
                        if draft.hasSavedPassword {
                            Toggle(L("Change saved password", "修改已保存的密码"), isOn: $draft.replaceSavedPassword)
                                .accessibilityIdentifier("replaceSavedPassword")
                        }
                        if !draft.hasSavedPassword || draft.replaceSavedPassword {
                            ConnectionField(title: draft.useRelay ? L("Relay access password", "中继访问密码") : L("Password", "密码")) {
                                SecureField(draft.useRelay ? L("Optional", "选填") : L("Password", "密码"), text: $draft.password)
                                    .focused($focused, equals: .password)
                                    .textContentType(.password).accessibilityIdentifier("connectionPassword")
                            }
                        }
                        if draft.protocol == .rdp {
                            ConnectionField(title: L("Domain", "域")) {
                                TextField(L("Optional", "选填"), text: $draft.domain)
                                    .focused($focused, equals: .domain)
                                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                            }
                            Toggle(L("Allow untrusted certificate", "允许不受信任的证书"), isOn: $draft.allowUntrustedCertificate)
                                .accessibilityIdentifier("allowUntrustedCertificate")
                        }
                    } header: { Text(L("Authentication", "身份验证")) } footer: {
                        Text(draft.hasSavedPassword && !draft.replaceSavedPassword
                            ? L("The saved password is kept for this computer and account.", "将保留此电脑和账号已保存的密码。")
                            : (draft.useRelay ? L("Provided by the relay administrator, if required.", "仅在中继要求时填写，由管理员提供。") : L("Saved credentials are stored in Keychain.", "保存的凭据存放在系统钥匙串中。")))
                    }
                }
                Section {
                    Toggle(draft.isPairingCode ? L("Remember this computer", "记住此电脑") : L("Save connection", "保存连接"), isOn: $saveConnection)
                        .accessibilityIdentifier("saveConnection")
                }
            }
            .modifier(MobileContentWidth())
            .scrollDismissesKeyboard(.interactively)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                VStack(spacing: 8) {
                    if let failure {
                        Label(failure, systemImage: "exclamationmark.circle")
                            .font(.footnote).foregroundStyle(.red).textSelection(.enabled)
                            .accessibilityIdentifier("connectionError")
                    }
                    Button(action: connect) {
                        Text(L("Connect", "连接"))
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(PrimaryActionStyle()).controlSize(.large)
                    .disabled(connecting || !hasHost).accessibilityIdentifier("connectButton")
                }
                .padding(.horizontal, 20).padding(.vertical, 12)
                .frame(maxWidth: 720).frame(maxWidth: .infinity)
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
                    }.labelStyle(.iconOnly).disabled(!hasHost || connecting || draft.isPairingCode)
                }
            }
            .onChange(of: draft.protocol) { _, proto in
                draft.port = String(proto.port); draft.useRelay = false; draft.password = ""
                draft.username = ""; draft.domain = ""; draft.allowUntrustedCertificate = false
            }
            .onChange(of: draft.host) { _, _ in draft.hostFingerprint = "" }
            .onChange(of: draft.relayEndpoint) { _, _ in draft.relayFingerprint = ""; draft.hostFingerprint = "" }
            .onChange(of: draft.relayTransport) { _, _ in draft.relayFingerprint = "" }
            .onChange(of: draft.relayServerName) { _, _ in draft.relayFingerprint = "" }
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
            if saveConnection && !draft.isPairingCode { try model.save(draft) }
            connecting = true; focused = nil
            var submitted = draft; submitted.rememberAfterPairing = saveConnection
            onConnect(submitted)
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
                    Toggle(isOn: $model.audioEnabled) { Label(L("Remote audio", "远程音频"), systemImage: "speaker.wave.2") }
                    Toggle(isOn: $model.clipboardEnabled) { Label(L("Text clipboard", "文本剪贴板"), systemImage: "doc.on.clipboard") }
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
                    DisclosureGroup {
                        Text(model.fingerprint).font(.footnote.monospaced()).textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    } label: {
                        Label(L("Certificate fingerprint", "证书指纹"), systemImage: "key.horizontal")
                    }
                }
                Section {
                    LabeledContent(L("Version", "版本"), value: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "")
                } footer: {
                    Text(L("Removent for iPhone and iPad", "Removent · iPhone 与 iPad"))
                }
            }
            .modifier(MobileContentWidth())
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
