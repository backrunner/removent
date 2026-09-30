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
    @FocusState private var pinFocused: Bool

    var body: some View {
        NavigationStack {
            ZStack(alignment: .topLeading) {
                RemoteCanvas(model: model, keyboard: $keyboard, modifiers: $modifiers)
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
                    .padding(12)
                }
            }
            .background(.black)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if model.ready { controls.padding(.horizontal, 12).padding(.vertical, 8) }
            }
            .navigationBarTitleDisplayMode(.inline)
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
                    Menu {
                        Toggle(L("Performance", "性能信息"), systemImage: "chart.bar", isOn: $stats)
                        if model.remoteAudio { Toggle(L("Mute audio", "静音"), systemImage: "speaker.slash", isOn: $model.muted) }
                        if model.remoteClipboard {
                            Button(L("Clipboard", "剪贴板"), systemImage: "doc.on.clipboard") { keyboard = false; clipboard = true }
                        }
                        if model.canRefresh {
                            Button(L("Refresh picture", "刷新画面"), systemImage: "arrow.clockwise") { model.refresh() }
                        }
                        Button(L("Touch gestures", "触控手势"), systemImage: "hand.draw") { keyboard = false; gestures = true }
                    } label: { Image(systemName: "ellipsis") }
                        .accessibilityLabel(L("Session actions", "会话操作"))
                        .disabled(!model.ready).accessibilityIdentifier("sessionMenu")
                }
            }
        }
        .onChange(of: model.ready) { _, ready in
            if !ready { keyboard = false; modifiers = 0; clipboard = false }
        }
        .onChange(of: model.needsPIN) { _, _ in pin = ""; pinFocused = false }
        .sheet(isPresented: $clipboard) { clipboardSheet }
        .sheet(isPresented: $gestures) { gestureSheet }
        // The desktop canvas is always black; keep its navigation and controls
        // legible even when the rest of the app follows a light appearance.
        .preferredColorScheme(.dark)
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
                .frame(maxWidth: .infinity, minHeight: geometry.size.height)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color(uiColor: .systemGroupedBackground))
        }
    }

    private var controls: some View {
        HStack(spacing: 4) {
            Button { keyboard.toggle() } label: { Image(systemName: keyboard ? "keyboard.chevron.compact.down" : "keyboard") }
                .frame(minWidth: 48, minHeight: 44)
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
        .font(.body.weight(.medium)).buttonStyle(.plain).foregroundStyle(.primary)
        .padding(8).modifier(SessionGlass())
        .frame(maxWidth: 780)
    }
    private func modifier(_ title: String, name: String, bit: Int) -> some View {
        let selected = modifiers & bit != 0
        return Button(title) { modifiers ^= bit }
            .padding(.horizontal, 8).frame(minWidth: 44, minHeight: 44)
            .background(selected ? Color.accentColor.opacity(0.22) : .clear, in: RoundedRectangle(cornerRadius: 12))
            .overlay(alignment: .bottom) {
                if selected { Circle().fill(Color.accentColor).frame(width: 4, height: 4).padding(.bottom, 3) }
            }
            .accessibilityLabel(name).accessibilityValue(selected ? L("On", "已开启") : L("Off", "已关闭"))
            .accessibilityAddTraits(selected ? .isSelected : [])
    }
    private func key(_ title: String, name: String, code: Int) -> some View {
        Button(title) { model.key(code, modifiers: modifiers) }
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
            .navigationTitle(L("Clipboard", "剪贴板")).navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button(L("Done", "完成")) { clipboard = false } } }
        }
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
            .navigationTitle(L("Touch gestures", "触控手势")).navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button(L("Done", "完成")) { gestures = false } } }
        }
    }
}

private struct SessionGlass: ViewModifier {
    func body(content: Content) -> some View {
        content.glassEffect(.regular, in: RoundedRectangle(cornerRadius: 24))
    }
}
