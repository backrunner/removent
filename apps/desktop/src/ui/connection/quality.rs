use super::*;
use removent_proto::VideoQuality;

pub(crate) fn quality_label(quality: VideoQuality) -> String {
    t!(match quality {
        VideoQuality::Low => "connection.quality_low",
        VideoQuality::Standard => "connection.quality_standard",
        VideoQuality::High => "connection.quality_high",
        VideoQuality::Ultra => "connection.quality_ultra",
    })
    .to_string()
}

impl ConnectionDialog {
    pub(super) fn render_quality(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut choices = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .p(px(3.))
            .w_full()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(8.))
            .bg(cx.theme().background);
        for (index, quality) in VideoQuality::ALL.into_iter().enumerate() {
            choices = choices.child(
                Button::new(("connection-quality", index))
                    .debug_selector(move || format!("connection-quality-{index}"))
                    .small()
                    .flex_1()
                    .min_w_0()
                    .h(px(28.))
                    .rounded(px(5.))
                    .segment(self.video_quality == quality)
                    .disabled(self.connecting)
                    .label(quality_label(quality))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.video_quality = quality;
                        cx.notify();
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .child(t!("connection.video_quality").to_string()),
            )
            .child(choices)
    }
}
