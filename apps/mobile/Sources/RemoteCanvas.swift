import SwiftUI
import UIKit

struct RemoteCanvas: UIViewRepresentable {
    let model: ControllerModel
    @Binding var keyboard: Bool
    @Binding var modifiers: Int
    func makeUIView(context: Context) -> CanvasView {
        let view = CanvasView()
        view.model = model
        return view
    }
    func updateUIView(_ view: CanvasView, context: Context) {
        view.setConnected(model.ready)
        view.modifiers = modifiers
        view.keyboard.wantsKeyboard = keyboard && model.ready
        if view.keyboard.wantsKeyboard && !view.keyboard.isFirstResponder { view.keyboard.becomeFirstResponder() }
        if !view.keyboard.wantsKeyboard && view.keyboard.isFirstResponder { _ = view.keyboard.resignFirstResponder() }
        model.onFrame = { [weak view] image in view?.show(image) }
    }
    static func dismantleUIView(_ uiView: CanvasView, coordinator: ()) { uiView.model?.onFrame = nil }
}

/// Coordinates are always measured in the image view's unzoomed coordinate space.
/// Pinch zoom and three-finger pan are local; two-finger pan scrolls the remote.
@MainActor
final class CanvasView: UIView, UIScrollViewDelegate, UIGestureRecognizerDelegate {
    weak var model: ControllerModel?
    var modifiers = 0
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
        keyboard.backgroundColor = .clear; keyboard.textColor = .clear; keyboard.tintColor = .clear
        keyboard.autocorrectionType = .no; keyboard.autocapitalizationType = .none
        keyboard.smartQuotesType = .no; keyboard.smartDashesType = .no
        keyboard.isAccessibilityElement = false; addSubview(keyboard)
        keyboard.sendText = { [weak self] text in
            guard let self else { return }
            if self.modifiers != 0, text.count == 1, let scalar = text.unicodeScalars.first,
                let code = RemoteKeyboard.macKey(for:scalar) { self.model?.key(code, modifiers:self.modifiers) }
            else { self.model?.text(text) }
        }
        keyboard.sendKey = { [weak self] code, mods, down in
            guard let self else { return }
            self.model?.input(["kind":"key", "code":code,
                "modifiers":mods | (down ? self.modifiers : 0), "down":down])
        }
        keyboard.backspace = { [weak self] in self?.model?.key(51, modifiers:self?.modifiers ?? 0) }
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
    }
    required init?(coder:NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func layoutSubviews() {
        super.layoutSubviews()
        if scroll.frame.size != bounds.size {
            scroll.zoomScale = 1; scroll.frame = bounds; fit()
        }
        keyboard.frame = CGRect(x:0,y:0,width:1,height:1)
    }
    func show(_ frame: CGImage) {
        let size = CGSize(width:frame.width, height:frame.height)
        if remoteSize != size { remoteSize = size; scroll.zoomScale = 1; fit() }
        image.image = UIImage(cgImage:frame)
    }
    private func fit() {
        guard remoteSize.width > 0, bounds.width > 0, bounds.height > 0 else { return }
        let scale = min(bounds.width / remoteSize.width, bounds.height / remoteSize.height)
        image.frame = CGRect(origin:.zero, size:CGSize(width:remoteSize.width * scale, height:remoteSize.height * scale))
        scroll.contentSize = image.frame.size; centerImage()
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
        guard let p = point(gesture) else { return }
        send(p, buttons:1, action:"LeftDown"); send(p, buttons:0, action:"LeftUp")
    }
    @objc private func rightTap(_ gesture:UITapGestureRecognizer) {
        guard let p = point(gesture) else { return }
        send(p, buttons:2, action:"RightDown"); send(p, buttons:0, action:"RightUp")
    }
    @objc private func drag(_ gesture:UILongPressGestureRecognizer) {
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
        let delta = gesture.translation(in:self); gesture.setTranslation(.zero, in:self)
        let phase = gesture.state == .began ? "Began" : (gesture.state == .ended || gesture.state == .cancelled) ? "Ended" : "Changed"
        model?.input(["kind":"scroll", "dx":-delta.x / 4, "dy":-delta.y / 4, "phase":phase])
    }
    @objc private func hover(_ gesture:UIHoverGestureRecognizer) {
        guard let p = point(gesture) else { return }; send(p, buttons:0, action:"Moved")
    }
}

@MainActor
final class RemoteKeyboard: UITextView, UITextViewDelegate {
    var wantsKeyboard = false
    var sendText: ((String)->Void)?
    var sendKey: ((Int,Int,Bool)->Void)?
    var backspace: (()->Void)?
    private let sentinel = "\u{200B}"
    private var pressed: [UIKeyboardHIDUsage:(Int,Int)] = [:]
    override var canBecomeFirstResponder: Bool { wantsKeyboard }
    override init(frame:CGRect, textContainer:NSTextContainer?) {
        super.init(frame:frame,textContainer:textContainer); delegate = self; text = sentinel
    }
    required init?(coder:NSCoder) { fatalError("init(coder:) has not been implemented") }
    func textViewDidChange(_ textView:UITextView) {
        // Wait for the IME to commit before forwarding text to the computer.
        guard markedTextRange == nil else { return }
        let committed = text.replacingOccurrences(of:sentinel,with:"")
        if !committed.isEmpty { sendText?(committed) }
        text = sentinel
    }
    func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
        guard markedTextRange == nil, text == "\n" || text == "\t" else { return true }
        let code = text == "\n" ? 36 : 48
        sendKey?(code, 0, true); sendKey?(code, 0, false)
        return false
    }
    override func deleteBackward() {
        // Deleting inside an IME composition must not delete remote text.
        if markedTextRange != nil { super.deleteBackward(); return }
        backspace?(); text = sentinel
    }
    override func pressesBegan(_ presses:Set<UIPress>, with event:UIPressesEvent?) {
        var remaining = Set<UIPress>()
        for press in presses {
            guard let key = press.key else { remaining.insert(press); continue }
            let mods = Self.modifiers(key.modifierFlags)
            let code = Self.special(key.keyCode) ?? ((mods & (4|16)) != 0 ? key.charactersIgnoringModifiers.unicodeScalars.first.flatMap(Self.macKey) : nil)
            if let code { pressed[key.keyCode] = (code,mods); sendKey?(code,mods,true) }
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
    static func macKey(for scalar:Unicode.Scalar) -> Int? {
        let keys: [Character:Int] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,
            "q":12,"w":13,"e":14,"r":15,"y":16,"t":17,"1":18,"2":19,"3":20,"4":21,"6":22,"5":23,
            "=":24,"9":25,"7":26,"-":27,"8":28,"0":29,"]":30,"o":31,"u":32,"[":33,"i":34,"p":35,
            "l":37,"j":38,"'":39,"k":40,";":41,"\\":42,",":43,"/":44,"n":45,"m":46,".":47," ":49,"`":50]
        let characters = Array(String(scalar).lowercased())
        guard characters.count == 1 else { return nil }
        return keys[characters[0]]
    }
}
