import SwiftUI

struct SessionView: View {
    @ObservedObject var model: ControllerModel
    @State private var keyboard = false
    @State private var stats = false
    @State private var pin = ""
    @State private var modifiers = 0
    @State private var clipboard = false
    @State private var clipboardText = ""
    @State private var gestures = false
    @State private var actions = false
    @State private var actionsPresented = false
    private enum PendingSheet { case clipboard, gestures }
    @State private var pendingSheet: PendingSheet?
    @State private var controlsVisible = true
    @State private var activity = UUID()
    @State private var keyboardFrame = CGRect.zero
    @State private var windowSize = CGSize.zero
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @FocusState private var pinFocused: Bool

    var body: some View {
        NavigationStack {
            GeometryReader { safeGeometry in
                GeometryReader { geometry in
                    // The actual window stays full-sized when UIKit changes its
                    // keyboard/home-indicator insets, but follows real rotation
                    // and iPad window resizing, including hardware keyboard use.
                    let viewport = Self.viewportSize(available: geometry.size, window: windowSize, ready: model.ready)
                    let landscape = windowSize.width > 0 && abs(windowSize.width - geometry.size.width) < 1 ?
                        windowSize.width > windowSize.height : geometry.size.width > geometry.size.height
                    let viewportHeight = viewport.height
                    let frame = CGRect(origin: geometry.frame(in: .global).origin, size: viewport)
                    let keyboardHeight = keyboardFrame.maxY >= frame.maxY - 1 ? max(0, frame.maxY - max(frame.minY, keyboardFrame.minY)) : 0
                    ZStack(alignment: .topLeading) {
                        RemoteCanvas(model: model, keyboard: $keyboard, modifiers: $modifiers,
                                     fillsScreen: model.ready && landscape,
                                     keyboardFocusAllowed: !actions && !actionsPresented && !clipboard && !gestures && pendingSheet == nil,
                                     onInteraction: revealControls,
                                     onWindowSizeChange: { windowSize = $0 },
                                     onKeyboardFrameChange: { keyboardFrame = $0 })
                            .frame(width: viewport.width, height: max(1, viewportHeight - (landscape && model.ready ? 0 : keyboardHeight)))
                            .accessibilityIdentifier("remoteCanvas")
                            .accessibilityLabel(L("Remote desktop", "远程桌面"))
                            .accessibilityValue("\(model.receivedFrames)")
                        if !model.ready { connectionState }
                        if model.ready && (stats || model.error != nil) {
                            VStack(alignment: .leading, spacing: 8) {
                                if stats {
                                    Text("\(Int(model.frameSize.width)) × \(Int(model.frameSize.height)) · \(model.fps) fps · \(model.codec)\n\(model.receivedFrames) \(L("frames", "帧"))")
                                        .font(.caption.monospacedDigit())
                                }
                                if let error = model.error {
                                    HStack(alignment: .top) {
                                        Label(error, systemImage: "exclamationmark.circle").font(.footnote)
                                        Button(L("Dismiss", "关闭提示"), systemImage: "xmark") { model.error = nil }
                                            .labelStyle(.iconOnly).frame(minWidth: 44, minHeight: 44)
                                    }
                                }
                            }
                            .padding(12).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 16))
                            .padding(12).padding(.top, model.ready ? safeGeometry.safeAreaInsets.top + 64 : 0)
                        }
                        if model.ready {
                            VStack {
                                sessionHeader
                                Spacer(minLength: 0)
                                controls
                            }
                            .padding(.horizontal, max(12, max(safeGeometry.safeAreaInsets.leading, safeGeometry.safeAreaInsets.trailing)))
                            .padding(.top, safeGeometry.safeAreaInsets.top + 8)
                            .padding(.bottom, max(safeGeometry.safeAreaInsets.bottom, keyboardHeight) + 8)
                            .opacity(controlsVisible ? 1 : 0)
                            .allowsHitTesting(controlsVisible)
                            .accessibilityHidden(!controlsVisible)
                            if !controlsVisible {
                                Button(action: revealControls) {
                                    Image(systemName: "ellipsis").frame(width: 44, height: 44).contentShape(Rectangle())
                                }
                                    .modifier(SessionGlass())
                                    .padding(.top, safeGeometry.safeAreaInsets.top + 8)
                                    .padding(.trailing, max(12, safeGeometry.safeAreaInsets.trailing))
                                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                                    .accessibilityLabel(L("Show controls", "显示操作栏"))
                                    .accessibilityIdentifier("showSessionControls")
                            }
                        }
                    }
                    .frame(width: viewport.width, height: viewportHeight, alignment: .topLeading)
                    .background(.black)
                }
                .ignoresSafeArea(.container, edges: model.ready ? .all : [])
            }
            .ignoresSafeArea(.keyboard, edges: model.ready ? .all : [])
            .navigationBarTitleDisplayMode(.inline)
            .toolbar(model.ready ? .hidden : .visible, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { model.disconnect() } label: { Image(systemName: "xmark") }
                        .accessibilityLabel(L("Disconnect", "断开连接"))
                        .accessibilityIdentifier("disconnectButton")
                }
                ToolbarItem(placement: .principal) {
                    VStack(spacing: 2) {
                        Text(model.sessionTitle).font(.headline).lineLimit(1)
                        Text(model.status).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                            .accessibilityIdentifier("sessionStatus")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    actionsButton
                }
            }
        }
        .ignoresSafeArea(.keyboard, edges: model.ready ? .all : [])
        .onChange(of: model.ready) { _, ready in
            if !ready {
                keyboard = false; modifiers = 0; keyboardFrame = .zero
                clipboard = false; gestures = false; actions = false; actionsPresented = false
                pendingSheet = nil
            }
            revealControls()
        }
        .onChange(of: keyboard) { _, _ in revealControls() }
        .onChange(of: actions) { _, value in
            if value { actionsPresented = true }
            revealControls()
        }
        .onChange(of: clipboard) { _, _ in revealControls() }
        .onChange(of: gestures) { _, _ in revealControls() }
        .onReceive(NotificationCenter.default.publisher(for: UIAccessibility.voiceOverStatusDidChangeNotification)) { _ in revealControls() }
        .task(id: activity) {
            guard model.ready, !keyboard, !actions, !actionsPresented, !clipboard, !gestures, !UIAccessibility.isVoiceOverRunning else { return }
            do { try await Task.sleep(for: .seconds(4)) } catch { return }
            guard !Task.isCancelled, model.ready, !keyboard, !actions, !actionsPresented, !clipboard, !gestures, !UIAccessibility.isVoiceOverRunning else { return }
            withAnimation(reduceMotion ? nil : .easeOut(duration: 0.3)) { controlsVisible = false }
        }
        .onChange(of: model.needsPIN) { _, _ in pin = ""; pinFocused = false }
        .sheet(isPresented: $clipboard) { clipboardSheet }
        .sheet(isPresented: $gestures) { gestureSheet }
        .sheet(isPresented: $actions, onDismiss: finishSessionAction) { sessionActionsSheet }
        // The keyboard leaves little vertical room for session controls in
        // landscape. Keep those targets usable; forms retain the user's size.
        .dynamicTypeSize(model.ready ? min(dynamicTypeSize, .xxxLarge) : dynamicTypeSize)
        .statusBarHidden(model.ready)
        // The desktop canvas is always black; keep its navigation and controls
        // legible even when the rest of the app follows a light appearance.
        .preferredColorScheme(.dark)
    }

    static func viewportSize(available: CGSize, window: CGSize, ready: Bool) -> CGSize {
        let fallback = CGSize(width: available.width.isFinite ? max(1, available.width) : 1,
                              height: available.height.isFinite ? max(1, available.height) : 1)
        guard ready, abs(window.width - available.width) < 1, window.width > window.height,
              window.width > 0, window.height > 0 else { return fallback }
        return window
    }

    private func revealControls() {
        withAnimation(reduceMotion ? nil : .easeOut(duration: 0.2)) { controlsVisible = true }
        activity = UUID()
    }
    private func finishSessionAction() {
        actionsPresented = false
        let destination = pendingSheet
        pendingSheet = nil
        revealControls()
        guard model.ready else { return }
        switch destination {
        case .clipboard: clipboard = true
        case .gestures: gestures = true
        case nil: break
        }
    }
    private var actionsButton: some View {
        Button { actions = true } label: {
            Image(systemName: "ellipsis").frame(width: 44, height: 44).contentShape(Rectangle())
        }
            .accessibilityLabel(L("Session actions", "会话操作"))
            .disabled(!model.ready).accessibilityIdentifier("sessionMenu")
    }
    private var sessionHeader: some View {
        HStack {
            Button { model.disconnect() } label: {
                Image(systemName: "xmark").frame(width: 44, height: 44).contentShape(Rectangle())
            }
                .accessibilityLabel(L("Disconnect", "断开连接"))
                .accessibilityIdentifier("disconnectButton")
            Spacer(minLength: 8)
            VStack(spacing: 2) {
                Text(model.sessionTitle).font(.headline).lineLimit(1)
                Text(model.status).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    .accessibilityIdentifier("sessionStatus")
            }
            Spacer(minLength: 8)
            actionsButton
        }
        .padding(6).modifier(SessionGlass()).frame(maxWidth: 780)
        .accessibilityElement(children: .contain)
    }

    private var connectionState: some View {
        GeometryReader { geometry in
            ScrollView {
                VStack(spacing: 20) {
                    if model.needsPIN {
                        Image(systemName: "lock.shield").font(.system(size: 44)).foregroundStyle(.tint)
                            .accessibilityHidden(true)
                        Text(L("Pair with computer", "与电脑配对")).font(.title2.bold())
                        Text(model.status).foregroundStyle(.secondary).multilineTextAlignment(.center)
                        TextField(L("Six-digit PIN", "六位配对码"), text: $pin)
                            .font(.title.monospacedDigit()).multilineTextAlignment(.center)
                            .keyboardType(.numberPad).textContentType(.oneTimeCode).focused($pinFocused)
                            .padding(14).background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 16))
                            .accessibilityIdentifier("pairingPIN")
                            .onChange(of: pin) { _, value in
                                pin = String(value.filter { $0.isASCII && $0.isNumber }.prefix(6))
                            }
                        Button(L("Pair", "配对")) { pinFocused = false; model.submitPIN(pin) }
                            .buttonStyle(PrimaryActionStyle()).controlSize(.large).disabled(pin.count != 6)
                    } else if model.isConnecting {
                        ProgressView().controlSize(.large)
                        Text(model.status).font(.headline).multilineTextAlignment(.center)
                    } else {
                        Image(systemName: "network.slash").font(.system(size: 44)).foregroundStyle(.secondary)
                            .accessibilityHidden(true)
                        Text(model.status).font(.title2.bold()).multilineTextAlignment(.center)
                        Button(L("Reconnect", "重新连接"), systemImage: "arrow.clockwise") { model.retry() }
                            .buttonStyle(PrimaryActionStyle()).controlSize(.large)
                    }
                    if let error = model.error {
                        Label(error, systemImage: "exclamationmark.circle")
                            .font(.footnote).foregroundStyle(.red).textSelection(.enabled)
                    }
                }
                .frame(maxWidth: 420).padding(24)
                .frame(maxWidth: .infinity, minHeight: geometry.size.height.isFinite ? max(0, geometry.size.height) : 0)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color(uiColor: .systemGroupedBackground))
        }
    }

    private var controls: some View {
        HStack(spacing: 4) {
            Button { keyboard.toggle() } label: {
                Image(systemName: keyboard ? "keyboard.chevron.compact.down" : "keyboard")
                    .frame(width: 48, height: 44).contentShape(Rectangle())
            }
                .accessibilityLabel(L("Keyboard", "键盘"))
                .accessibilityValue(keyboard ? L("Shown", "已显示") : L("Hidden", "已隐藏"))
                .accessibilityIdentifier("remoteKeyboard")
            Divider().frame(height: 24)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 4) {
                    modifier("⌘", name: L("Command", "Command 键"), bit: 16)
                    modifier("Ctrl", name: L("Control", "Control 键"), bit: 4)
                    modifier("⌥", name: L("Option", "Option 键"), bit: 8)
                    modifier("⇧", name: L("Shift", "Shift 键"), bit: 2)
                    key("Esc", name: L("Escape", "Escape 键"), code: 53)
                    key("Tab", name: L("Tab", "制表键"), code: 48)
                    key("↵", name: L("Return", "回车键"), code: 36)
                    key("←", name: L("Left arrow", "向左"), code: 123)
                    key("↑", name: L("Up arrow", "向上"), code: 126)
                    key("↓", name: L("Down arrow", "向下"), code: 125)
                    key("→", name: L("Right arrow", "向右"), code: 124)
                }
            }
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("remoteKeys")
        }
        .font(.callout.weight(.medium)).buttonStyle(.plain).foregroundStyle(.primary)
        .padding(6).modifier(SessionGlass())
        .frame(maxWidth: 780)
        .accessibilityElement(children: .contain)
    }
    private func modifier(_ title: String, name: String, bit: Int) -> some View {
        let selected = modifiers & bit != 0
        return Button(title) { modifiers ^= bit; revealControls() }
            .padding(.horizontal, 8).frame(minWidth: 44, minHeight: 44)
            .background(selected ? Color.accentColor.opacity(0.22) : .clear, in: RoundedRectangle(cornerRadius: 12))
            .overlay(alignment: .bottom) {
                if selected { Circle().fill(Color.accentColor).frame(width: 4, height: 4).padding(.bottom, 3) }
            }
            .accessibilityLabel(name).accessibilityValue(selected ? L("On", "已开启") : L("Off", "已关闭"))
            .accessibilityAddTraits(selected ? .isSelected : [])
    }
    private func key(_ title: String, name: String, code: Int) -> some View {
        Button(title) { model.key(code, modifiers: modifiers); revealControls() }
            .padding(.horizontal, 8).frame(minWidth: 44, minHeight: 44)
            .accessibilityLabel(name).accessibilityIdentifier(title)
    }

    private var clipboardSheet: some View {
        NavigationStack {
            Form {
                Section(L("Send to computer", "发送到电脑")) {
                    TextEditor(text: $clipboardText).frame(minHeight: 140)
                        .accessibilityLabel(L("Clipboard text", "剪贴板文本"))
                    PasteButton(payloadType: String.self) { strings in clipboardText = strings.joined(separator: "\n") }
                    Button(L("Send text", "发送文本")) {
                        if model.sendClipboard(clipboardText) { clipboard = false }
                    }
                }
                Section {
                    Button(L("Copy computer clipboard", "复制电脑剪贴板")) {
                        if model.copyRemoteClipboard() { clipboard = false }
                    }
                }
                if let error = model.error {
                    Section { Label(error, systemImage: "exclamationmark.circle").foregroundStyle(.red) }
                }
            }
            .modifier(SessionContentWidth())
            .navigationTitle(L("Clipboard", "剪贴板")).navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button(L("Done", "完成")) { clipboard = false } } }
        }
        .dynamicTypeSize(dynamicTypeSize)
    }
    private var sessionActionsSheet: some View {
        NavigationStack {
            List {
                Button(stats ? L("Hide performance", "隐藏性能信息") : L("Performance", "性能信息")) {
                    stats.toggle(); actions = false
                }
                if model.remoteAudio {
                    Button(model.muted ? L("Unmute audio", "取消静音") : L("Mute audio", "静音")) {
                        model.muted.toggle(); actions = false
                    }
                }
                if model.remoteClipboard {
                    Button(L("Clipboard", "剪贴板")) { keyboard = false; pendingSheet = .clipboard; actions = false }
                }
                if model.canRefresh {
                    Button(L("Refresh picture", "刷新画面")) { model.refresh(); actions = false }
                }
                Button(L("Touch gestures", "触控手势")) { keyboard = false; pendingSheet = .gestures; actions = false }
            }
            .accessibilityIdentifier("sessionActionsList")
            .modifier(SessionContentWidth())
            .navigationTitle(L("Session actions", "会话操作")).navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button(L("Done", "完成")) { actions = false } } }
        }
        .dynamicTypeSize(dynamicTypeSize)
    }
    private var gestureSheet: some View {
        NavigationStack {
            List {
                LabeledContent(L("Click", "单击"), value: L("Tap", "轻点"))
                LabeledContent(L("Right-click", "右键"), value: L("Two-finger tap", "双指轻点"))
                LabeledContent(L("Drag", "拖动"), value: L("Touch and hold, then move", "长按后移动"))
                LabeledContent(L("Scroll computer", "滚动电脑页面"), value: L("Two-finger swipe", "双指滑动"))
                LabeledContent(L("Zoom", "缩放"), value: L("Pinch", "双指捏合"))
                LabeledContent(L("Pan zoomed picture", "平移放大画面"), value: L("Three-finger swipe", "三指滑动"))
            }
            .modifier(SessionContentWidth())
            .navigationTitle(L("Touch gestures", "触控手势")).navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button(L("Done", "完成")) { gestures = false } } }
        }
        .dynamicTypeSize(dynamicTypeSize)
    }
}

private struct SessionGlass: ViewModifier {
    func body(content: Content) -> some View {
        content.glassEffect(.regular, in: RoundedRectangle(cornerRadius: 24))
    }
}

private struct SessionContentWidth: ViewModifier {
    func body(content: Content) -> some View {
        content.frame(maxWidth: 720).frame(maxWidth: .infinity)
            .background(Color(uiColor: .systemGroupedBackground))
    }
}
