import SwiftUI
import UIKit

struct RemoteCanvas: UIViewRepresentable {
    let model: ControllerModel
    @Binding var keyboard: Bool
    @Binding var modifiers: Int
    var fillsScreen = false
    var keyboardFocusAllowed = true
    var onInteraction: () -> Void = {}
    var onWindowSizeChange: (CGSize) -> Void = { _ in }
    var onKeyboardFrameChange: (CGRect) -> Void = { _ in }
    func makeUIView(context: Context) -> CanvasView {
        let view = CanvasView()
        view.model = model
        return view
    }
    func updateUIView(_ view: CanvasView, context: Context) {
        view.setConnected(model.ready)
        view.modifiers = modifiers
        view.fillsScreen = fillsScreen
        view.onInteraction = onInteraction
        view.onWindowSizeChange = onWindowSizeChange
        view.onKeyboardFrameChange = onKeyboardFrameChange
        view.keyboard.additionalModifiers = modifiers
        view.keyboard.dismiss = { keyboard = false }
        view.keyboard.wantsKeyboard = keyboard && model.ready && keyboardFocusAllowed
        if view.keyboard.wantsKeyboard && !view.keyboard.isFirstResponder && view.window?.isKeyWindow != false { view.keyboard.becomeFirstResponder() }
        if !view.keyboard.wantsKeyboard && view.keyboard.isFirstResponder { _ = view.keyboard.resignFirstResponder() }
        model.onFrame = { [weak view] image in view?.show(image) }
    }
    static func dismantleUIView(_ uiView: CanvasView, coordinator: ()) {
        uiView.onInteraction = nil
        uiView.onWindowSizeChange = nil
        uiView.onKeyboardFrameChange = nil
        _ = uiView.keyboard.resignFirstResponder()
        uiView.model?.onFrame = nil
    }
}

/// A session-only input view. No globe, dictation, prediction or local text buffer.
@MainActor
final class EnglishKeyboardView: UIInputView {
    private weak var receiver: RemoteKeyboard?
    private let rows = UIStackView()
    private var page = 0
    private var uppercase = false
    private var sizingWidth: CGFloat = 0

    init(receiver: RemoteKeyboard) {
        self.receiver = receiver
        super.init(frame: CGRect(x: 0, y: 0, width: 0, height: 260), inputViewStyle: .keyboard)
        allowsSelfSizing = true
        accessibilityIdentifier = "englishRemoteKeyboard"
        rows.axis = .vertical; rows.spacing = 6; rows.distribution = .fillEqually
        rows.translatesAutoresizingMaskIntoConstraints = false
        addSubview(rows)
        NSLayoutConstraint.activate([
            rows.topAnchor.constraint(equalTo: topAnchor, constant: 8),
            rows.bottomAnchor.constraint(equalTo: safeAreaLayoutGuide.bottomAnchor, constant: -8),
            rows.leadingAnchor.constraint(equalTo: safeAreaLayoutGuide.leadingAnchor, constant: 8),
            rows.trailingAnchor.constraint(equalTo: safeAreaLayoutGuide.trailingAnchor, constant: -8)
        ])
        rebuild()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override var intrinsicContentSize: CGSize {
        CGSize(width: UIView.noIntrinsicMetric, height: bounds.width > 600 && traitCollection.userInterfaceIdiom == .phone ? 240 : 260)
    }
    override func layoutSubviews() {
        super.layoutSubviews()
        if sizingWidth != bounds.width { sizingWidth = bounds.width; invalidateIntrinsicContentSize() }
    }

    private func button(_ title: String, id: String? = nil, label: String? = nil, action: @escaping () -> Void) -> UIButton {
        var config = UIButton.Configuration.filled()
        config.title = title; config.baseForegroundColor = .label
        // Like a system keyboard, key labels must fit the fixed-height rows.
        config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { attributes in
            var result = attributes
            result.font = .systemFont(ofSize: 20)
            return result
        }
        config.titleLineBreakMode = .byClipping
        config.baseBackgroundColor = .secondarySystemGroupedBackground
        config.cornerStyle = .medium
        let button = UIButton(configuration: config, primaryAction: UIAction(title: title) { _ in action() })
        button.titleLabel?.adjustsFontSizeToFitWidth = true
        button.titleLabel?.minimumScaleFactor = 0.6
        button.accessibilityIdentifier = id ?? "remoteKey-\(title)"
        button.accessibilityLabel = label ?? title
        return button
    }
    private func row() -> UIStackView {
        let row = UIStackView(); row.spacing = 5; row.distribution = .fillEqually
        rows.addArrangedSubview(row); return row
    }
    private func addCharacters(_ text: String, to row: UIStackView) {
        for char in text {
            let value = String(char)
            row.addArrangedSubview(button(value) { [weak self] in self?.receiver?.insertText(value) })
        }
    }
    private func rebuild() {
        for view in rows.arrangedSubviews { rows.removeArrangedSubview(view); view.removeFromSuperview() }
        let layouts = page == 0 ? ["qwertyuiop", "asdfghjkl", "zxcvbnm"] :
            page == 1 ? ["1234567890", "-/:;()$&@\"", ".,?!'"] :
            ["[]{}#%^*+=", "_\\|~<>`:\"", ".,?!'"]
        addCharacters(page == 0 && uppercase ? layouts[0].uppercased() : layouts[0], to: row())
        addCharacters(page == 0 && uppercase ? layouts[1].uppercased() : layouts[1], to: row())
        let third = row()
        let shift = button(page == 0 ? "⇧" : page == 1 ? "#+=" : "123", id: "remoteShift", label: page == 0 ? L("Shift", "Shift 键") : L("Symbols", "符号")) { [weak self] in
            guard let self else { return }
            if self.page == 0 { self.uppercase.toggle() } else { self.page = self.page == 1 ? 2 : 1 }
            self.rebuild()
        }
        if page == 0 && uppercase {
            shift.configuration?.baseBackgroundColor = .systemBlue
            shift.configuration?.baseForegroundColor = .white
            shift.accessibilityTraits.insert(.selected)
        }
        third.addArrangedSubview(shift)
        addCharacters(page == 0 && uppercase ? layouts[2].uppercased() : layouts[2], to: third)
        third.addArrangedSubview(button("⌫", id: "remoteBackspace", label: L("Delete", "删除")) { [weak self] in self?.receiver?.deleteBackward() })
        let bottom = row()
        bottom.addArrangedSubview(button(page == 0 ? "123" : "ABC", id: "remoteKeyboardPage") { [weak self] in
            guard let self else { return }; self.page = self.page == 0 ? 1 : 0; self.rebuild()
        })
        let space = button(L("space", "空格"), id: "remoteSpace") { [weak self] in self?.receiver?.insertText(" ") }
        bottom.addArrangedSubview(space)
        bottom.addArrangedSubview(button("↵", id: "remoteReturn", label: L("Return", "回车键")) { [weak self] in self?.receiver?.insertText("\n") })
        bottom.addArrangedSubview(button("⌄", id: "hideRemoteKeyboard", label: L("Hide keyboard", "收起键盘")) { [weak self] in self?.receiver?.dismiss?() })
        bottom.distribution = .fill
        for view in bottom.arrangedSubviews where view !== space { view.widthAnchor.constraint(equalTo: bottom.widthAnchor, multiplier: 0.16).isActive = true }
    }
}

/// Coordinates are always measured in the image view's unzoomed coordinate space.
/// Pinch zoom and three-finger pan are local; two-finger pan scrolls the remote.
@MainActor
final class CanvasView: UIView, UIScrollViewDelegate, UIGestureRecognizerDelegate {
    weak var model: ControllerModel?
    var modifiers = 0
    var onInteraction: (() -> Void)?
    var onWindowSizeChange: ((CGSize) -> Void)?
    var onKeyboardFrameChange: ((CGRect) -> Void)?
    private var reportedWindowSize = CGSize.zero
    private var keyboardObservers: [NSObjectProtocol] = []
    var fillsScreen = false {
        didSet { if oldValue != fillsScreen { scroll.zoomScale = 1; fit() } }
    }
    let keyboard = RemoteKeyboard()
    private let scroll = UIScrollView()
    private let image = UIImageView()
    private var remoteSize = CGSize.zero
    private var dragging = false
    private var lastDragPoint: CGPoint?
    private var connected = false

    func setConnected(_ value: Bool) {
        guard value != connected else { return }
        connected = value
        if !value {
            dragging = false; lastDragPoint = nil
            image.image = nil; remoteSize = .zero
            scroll.zoomScale = 1
        }
    }

    override init(frame: CGRect) {
        super.init(frame:frame)
        backgroundColor = .black
        scroll.delegate = self; scroll.minimumZoomScale = 1; scroll.maximumZoomScale = 4
        scroll.bouncesZoom = false; scroll.panGestureRecognizer.minimumNumberOfTouches = 3
        scroll.showsHorizontalScrollIndicator = false; scroll.showsVerticalScrollIndicator = false
        addSubview(scroll); scroll.addSubview(image)
        image.isUserInteractionEnabled = true
        image.layer.minificationFilter = .linear; image.layer.magnificationFilter = .linear
        keyboard.isAccessibilityElement = false; addSubview(keyboard)
        keyboard.sendText = { [weak self] text in
            guard let self else { return }
            self.onInteraction?()
            if self.modifiers != 0, text.count == 1, let scalar = text.unicodeScalars.first,
                let key = RemoteKeyboard.macKeyStroke(for: scalar) {
                self.model?.key(key.code, modifiers: self.modifiers | key.modifiers)
            }
            else { self.model?.text(text) }
        }
        keyboard.sendKey = { [weak self] code, mods, down in
            guard let self else { return }
            self.onInteraction?()
            self.model?.input(["kind":"key", "code":code,
                "modifiers":mods | (down ? self.modifiers : 0), "down":down])
        }
        keyboard.backspace = { [weak self] in
            self?.onInteraction?(); self?.model?.key(51, modifiers:self?.modifiers ?? 0)
        }
        let tap = UITapGestureRecognizer(target:self, action:#selector(tap(_:)))
        image.addGestureRecognizer(tap)
        let right = UITapGestureRecognizer(target:self, action:#selector(rightTap(_:)))
        right.numberOfTouchesRequired = 2; image.addGestureRecognizer(right)
        let drag = UILongPressGestureRecognizer(target:self, action:#selector(drag(_:)))
        drag.minimumPressDuration = 0.35; image.addGestureRecognizer(drag)
        tap.require(toFail:drag)
        let pan = UIPanGestureRecognizer(target:self, action:#selector(wheel(_:)))
        pan.minimumNumberOfTouches = 2; pan.maximumNumberOfTouches = 2
        pan.delegate = self; image.addGestureRecognizer(pan)
        let hover = UIHoverGestureRecognizer(target:self, action:#selector(hover(_:)))
        image.addGestureRecognizer(hover)
        // Trackpad/mouse primary clicks use the same point mapping; secondary click
        // is available through a separate allowed-button gesture.
        let secondary = UITapGestureRecognizer(target:self, action:#selector(rightTap(_:)))
        secondary.buttonMaskRequired = .secondary; image.addGestureRecognizer(secondary)
        for name in [UIResponder.keyboardWillChangeFrameNotification, UIResponder.keyboardWillHideNotification] {
            keyboardObservers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] notification in
                guard let self, let window = self.window else { return }
                guard window.isKeyWindow,
                      notification.userInfo?[UIResponder.keyboardIsLocalUserInfoKey] as? Bool != false else { return }
                let screenFrame = notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect ?? .zero
                let frame = name == UIResponder.keyboardWillHideNotification ? .zero :
                    window.screen.coordinateSpace.convert(screenFrame, to: window)
                DispatchQueue.main.async { [weak self] in self?.onKeyboardFrameChange?(frame) }
            })
        }
    }
    deinit { keyboardObservers.forEach(NotificationCenter.default.removeObserver) }
    required init?(coder:NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func didMoveToWindow() { super.didMoveToWindow(); reportWindowSize() }
    override func layoutSubviews() {
        super.layoutSubviews()
        reportWindowSize()
        if scroll.frame.size != bounds.size {
            scroll.zoomScale = 1; scroll.frame = bounds; fit()
        }
        keyboard.frame = CGRect(x:0,y:0,width:1,height:1)
    }
    private func reportWindowSize() {
        guard let size = window?.bounds.size, size != reportedWindowSize else { return }
        reportedWindowSize = size
        // UIKit layout can run during a SwiftUI update; publish after that pass.
        DispatchQueue.main.async { [weak self] in self?.onWindowSizeChange?(size) }
    }
    func show(_ frame: CGImage) {
        let size = CGSize(width:frame.width, height:frame.height)
        if remoteSize != size { remoteSize = size; scroll.zoomScale = 1; fit() }
        image.image = UIImage(cgImage:frame)
    }
    private func fit() {
        guard remoteSize.width > 0, bounds.width > 0, bounds.height > 0 else { return }
        let fitted = Self.displaySize(remoteSize: remoteSize, viewport: bounds.size, fill: fillsScreen)
        image.frame = CGRect(origin:.zero, size:fitted)
        scroll.contentSize = image.frame.size; centerImage()
        scroll.contentOffset = CGPoint(x:max(0,(fitted.width-bounds.width)/2) - scroll.contentInset.left,
                                       y:max(0,(fitted.height-bounds.height)/2) - scroll.contentInset.top)
    }
    static func displaySize(remoteSize: CGSize, viewport: CGSize, fill: Bool) -> CGSize {
        guard remoteSize.width > 0, remoteSize.height > 0, viewport.width > 0, viewport.height > 0 else { return .zero }
        let x = viewport.width / remoteSize.width, y = viewport.height / remoteSize.height
        let scale = fill ? max(x, y) : min(x, y)
        return CGSize(width: remoteSize.width * scale, height: remoteSize.height * scale)
    }
    private func centerImage() {
        let size = image.frame.size
        scroll.contentInset = UIEdgeInsets(top:max(0,(scroll.bounds.height-size.height)/2),
            left:max(0,(scroll.bounds.width-size.width)/2), bottom:0, right:0)
    }
    func viewForZooming(in scrollView:UIScrollView) -> UIView? { image }
    func scrollViewDidZoom(_ scrollView:UIScrollView) { centerImage() }
    private func point(_ gesture:UIGestureRecognizer) -> CGPoint? {
        Self.remotePoint(gesture.location(in:image), imageSize:image.bounds.size, remoteSize:remoteSize)
    }
    static func remotePoint(_ point:CGPoint, imageSize:CGSize, remoteSize:CGSize) -> CGPoint? {
        guard imageSize.width > 0, imageSize.height > 0, remoteSize.width > 0, remoteSize.height > 0,
            point.x.isFinite, point.y.isFinite else { return nil }
        return CGPoint(x:min(max(0,point.x / imageSize.width * remoteSize.width),remoteSize.width-1),
            y:min(max(0,point.y / imageSize.height * remoteSize.height),remoteSize.height-1))
    }
    private func send(_ point:CGPoint, buttons:Int, action:String) {
        model?.input(["kind":"pointer", "width":Int(remoteSize.width), "height":Int(remoteSize.height),
            "x":point.x, "y":point.y, "buttons":buttons, "action":action])
    }
    @objc private func tap(_ gesture:UITapGestureRecognizer) {
        onInteraction?()
        guard let p = point(gesture) else { return }
        send(p, buttons:1, action:"LeftDown"); send(p, buttons:0, action:"LeftUp")
    }
    @objc private func rightTap(_ gesture:UITapGestureRecognizer) {
        onInteraction?()
        guard let p = point(gesture) else { return }
        send(p, buttons:2, action:"RightDown"); send(p, buttons:0, action:"RightUp")
    }
    @objc private func drag(_ gesture:UILongPressGestureRecognizer) {
        onInteraction?()
        guard let p = point(gesture) ?? lastDragPoint else { return }
        switch gesture.state {
        case .began: dragging = true; lastDragPoint = p; send(p, buttons:1, action:"LeftDown")
        case .changed: lastDragPoint = p; send(p, buttons:1, action:"LeftDragged")
        case .ended, .cancelled, .failed:
            if dragging { send(p, buttons:0, action:"LeftUp") }; dragging = false; lastDragPoint = nil
        default: break
        }
    }
    @objc private func wheel(_ gesture:UIPanGestureRecognizer) {
        onInteraction?()
        let delta = gesture.translation(in:self); gesture.setTranslation(.zero, in:self)
        let phase = gesture.state == .began ? "Began" : (gesture.state == .ended || gesture.state == .cancelled) ? "Ended" : "Changed"
        model?.input(["kind":"scroll", "dx":-delta.x / 4, "dy":-delta.y / 4, "phase":phase])
    }
    @objc private func hover(_ gesture:UIHoverGestureRecognizer) {
        guard let p = point(gesture) else { return }; send(p, buttons:0, action:"Moved")
    }
}

@MainActor
final class RemoteKeyboard: UIView, UIKeyInput {
    var wantsKeyboard = false
    var additionalModifiers = 0
    var sendText: ((String)->Void)?
    var sendKey: ((Int,Int,Bool)->Void)?
    var backspace: (()->Void)?
    var dismiss: (() -> Void)?
    var hasText: Bool { true } // Backspace always targets the remote computer.
    private lazy var englishKeyboard = EnglishKeyboardView(receiver: self)
    override var inputView: UIView? { englishKeyboard }
    private var pressed: [UIKeyboardHIDUsage:(Int,Int)] = [:]
    override var canBecomeFirstResponder: Bool { wantsKeyboard }
    override init(frame:CGRect) { super.init(frame:frame) }
    required init?(coder:NSCoder) { fatalError("init(coder:) has not been implemented") }
    func insertText(_ text: String) {
        // There is no local text storage or marked-text/IME composition path.
        for scalar in text.unicodeScalars {
            switch scalar.value {
            case 10, 13: tapKey(36)
            case 9: tapKey(48)
            case 32...126: sendText?(String(scalar))
            default: break
            }
        }
    }
    func deleteBackward() { backspace?() }
    private func tapKey(_ code: Int) { sendKey?(code, 0, true); sendKey?(code, 0, false) }
    override func pressesBegan(_ presses:Set<UIPress>, with event:UIPressesEvent?) {
        var remaining = Set<UIPress>()
        for press in presses {
            guard let key = press.key else { remaining.insert(press); continue }
            let mods = Self.modifiers(key.modifierFlags)
            let character = Self.englishCharacter(key.keyCode, modifiers: key.modifierFlags)
            let code = Self.special(key.keyCode) ?? (((mods & (4|8|16)) != 0 || additionalModifiers != 0) ? Self.englishCharacter(key.keyCode, modifiers: []).flatMap { $0.unicodeScalars.first }.flatMap(Self.macKey) : nil)
            if let code { pressed[key.keyCode] = (code,mods); sendKey?(code,mods,true) }
            else if let character { insertText(character) }
            else { remaining.insert(press) }
        }
        if !remaining.isEmpty { super.pressesBegan(remaining,with:event) }
    }
    override func pressesEnded(_ presses:Set<UIPress>, with event:UIPressesEvent?) { release(presses,event:event) }
    override func pressesCancelled(_ presses:Set<UIPress>, with event:UIPressesEvent?) { release(presses,event:event) }
    private func release(_ presses:Set<UIPress>,event:UIPressesEvent?) {
        var remaining = Set<UIPress>()
        for press in presses {
            if let key = press.key, let (code,_) = pressed.removeValue(forKey:key.keyCode) { sendKey?(code,Self.modifiers(key.modifierFlags),false) }
            else { remaining.insert(press) }
        }
        if !remaining.isEmpty { super.pressesEnded(remaining,with:event) }
    }
    override func resignFirstResponder() -> Bool {
        for (code,_) in pressed.values { sendKey?(code,0,false) }; pressed.removeAll()
        return super.resignFirstResponder()
    }
    static func modifiers(_ flags:UIKeyModifierFlags) -> Int {
        (flags.contains(.shift) ? 2:0) | (flags.contains(.control) ? 4:0) |
        (flags.contains(.alternate) ? 8:0) | (flags.contains(.command) ? 16:0) |
        (flags.contains(.alphaShift) ? 1:0)
    }
    static func special(_ usage:UIKeyboardHIDUsage) -> Int? {
        switch usage {
        case .keyboardReturnOrEnter:36; case .keyboardEscape:53; case .keyboardDeleteOrBackspace:51
        case .keyboardTab:48; case .keyboardRightArrow:124; case .keyboardLeftArrow:123
        case .keyboardDownArrow:125; case .keyboardUpArrow:126; case .keyboardDeleteForward:117
        case .keyboardHome:115; case .keyboardEnd:119; case .keyboardPageUp:116; case .keyboardPageDown:121
        default:nil
        }
    }
    /// Physical US key positions avoid the device's selected hardware IME too.
    static func englishCharacter(_ usage: UIKeyboardHIDUsage, modifiers: UIKeyModifierFlags) -> String? {
        let value = Int(usage.rawValue)
        let shift = modifiers.contains(.shift)
        if (4...29).contains(value) {
            let uppercase = shift != modifiers.contains(.alphaShift)
            return String(Unicode.Scalar((uppercase ? 65 : 97) + value - 4)!)
        }
        if (30...39).contains(value) {
            return String(Array(shift ? "!@#$%^&*()" : "1234567890")[value - 30])
        }
        let punctuation = [44:(" "," "),45:("-","_"),46:("=","+"),47:("[","{"),48:("]","}"),49:("\\","|"),51:(";",":"),52:("'","\""),53:("`","~"),54:(",","<"),55:(".",">"),56:("/","?")]
        guard let pair = punctuation[value] else { return nil }
        return shift ? pair.1 : pair.0
    }
    static func macKey(for scalar:Unicode.Scalar) -> Int? {
        let keys: [Character:Int] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,
            "q":12,"w":13,"e":14,"r":15,"y":16,"t":17,"1":18,"2":19,"3":20,"4":21,"6":22,"5":23,
            "=":24,"9":25,"7":26,"-":27,"8":28,"0":29,"]":30,"o":31,"u":32,"[":33,"i":34,"p":35,
            "l":37,"j":38,"'":39,"k":40,";":41,"\\":42,",":43,"/":44,"n":45,"m":46,".":47," ":49,"`":50]
        let characters = Array(String(scalar).lowercased())
        guard characters.count == 1 else { return nil }
        return keys[characters[0]]
    }
    static func macKeyStroke(for scalar: Unicode.Scalar) -> (code: Int, modifiers: Int)? {
        guard scalar.isASCII else { return nil }
        if (65...90).contains(scalar.value), let code = macKey(for: scalar) { return (code, 2) }
        let shifted = Array("~!@#$%^&*()_+{}|:\"<>?")
        let unshifted = Array("`1234567890-=[]\\;',./")
        if let index = shifted.firstIndex(of: Character(String(scalar))),
           let base = String(unshifted[index]).unicodeScalars.first, let code = macKey(for: base) { return (code, 2) }
        return macKey(for: scalar).map { ($0, 0) }
    }
}
