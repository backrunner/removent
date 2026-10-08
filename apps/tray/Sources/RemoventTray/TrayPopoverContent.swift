import SwiftUI

struct TrayPopoverContent<Content: View>: View {
    let width: CGFloat
    let maximumHeight: CGFloat
    @ViewBuilder var content: () -> Content

    var body: some View {
        ViewThatFits(in: .vertical) {
            content().fixedSize(horizontal: false, vertical: true)
            ScrollView { content().fixedSize(horizontal: false, vertical: true) }
                .scrollBounceBehavior(.basedOnSize)
        }
        .frame(width: width)
        .frame(maxHeight: maximumHeight)
        // The maximum is a cap, not a request to fill the display's height.
        .fixedSize(horizontal: false, vertical: true)
    }
}
