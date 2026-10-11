//! Exercise both GPUI wrapping paths: measuring character widths and painting shaped glyphs.
use gpui::{
    IndentAdjustment, LineFragment, TestAppContext, TruncateFrom, WindowTextSystem, font, px,
};

fn check_boundary(text: &str, index: usize) {
    let previous = text[..index].trim_end().chars().next_back().unwrap();
    let current = text[index..].chars().next().unwrap();
    assert!(
        !"，。！？、；：）】》」』”’…".contains(current),
        "line starts with {current}: {text}, byte {index}"
    );
    assert!(
        !"（【《「『“‘".contains(previous),
        "line ends with {previous}: {text}, byte {index}"
    );
    assert!(!matches!((previous, current), ('—', '—') | ('…', '…')));
}

#[gpui::test]
fn chinese_punctuation_stays_attached_in_measurement_and_shaping(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let texts = [
            "在「 设置 → 局域网发现 」中开启协议，或手动添加连接。",
            "在「设置 → 局域网发现」中开启协议，或手动添加连接。",
            "检查局域网发现设置，并确认远端服务已在当前网络中运行。",
            "他说：“请连接（本机），然后重试！”设备【办公室】已就绪。",
            "等待……连接——请稍候；地址：192.168.1.11:5900。",
        ];
        let face = font(".SystemUIFont");
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        for text in texts {
            let run = gpui::TextRun {
                len: text.len(),
                font: face.clone(),
                color: Default::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let mut wrapper = cx.text_system().line_wrapper(face.clone(), px(11.));
            for width in 1..=240 {
                let fragments = [LineFragment::text(text)];
                for boundary in
                    wrapper.wrap_line(&fragments, px(width as f32), IndentAdjustment::NoIndent)
                {
                    check_boundary(text, boundary.ix);
                }
                for max_lines in 1..=3 {
                    let (truncated, _) = wrapper.truncate_wrapped_line(
                        text.to_owned().into(),
                        px(width as f32),
                        max_lines,
                        "…",
                        std::slice::from_ref(&run),
                        TruncateFrom::End,
                    );
                    if let Some(prefix) = truncated.strip_suffix('…') {
                        assert!(text.starts_with(prefix));
                        assert!(
                            prefix
                                .chars()
                                .next_back()
                                .is_none_or(|c| !"（【《「『“‘".contains(c))
                        );
                        let fragments = [LineFragment::text(prefix)];
                        for boundary in wrapper.wrap_line(
                            &fragments,
                            px(width as f32),
                            IndentAdjustment::NoIndent,
                        ) {
                            check_boundary(prefix, boundary.ix);
                        }
                    }
                }
                for clamp in [None, Some(2), Some(3)] {
                    let lines = text_system
                        .shape_text(
                            text.to_owned().into(),
                            px(11.),
                            std::slice::from_ref(&run),
                            Some(px(width as f32)),
                            clamp,
                        )
                        .unwrap();
                    for line in lines {
                        for boundary in line.wrap_boundaries() {
                            let index = line.unwrapped_layout.runs[boundary.run_ix].glyphs
                                [boundary.glyph_ix]
                                .index;
                            check_boundary(text, index);
                        }
                    }
                }
            }
        }
    });
}

#[gpui::test]
fn chinese_breaks_preserve_wrapping_and_explicit_newlines(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let text = "在「设置 → 局域网发现」中开启协议，或手动添加连接。\n没有匹配的设备";
        let face = font(".SystemUIFont");
        let run = gpui::TextRun {
            len: text.len(),
            font: face,
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let system = WindowTextSystem::new(cx.text_system().clone());
        let lines = system
            .shape_text(
                text.to_owned().into(),
                px(11.),
                &[run],
                Some(px(170.)),
                None,
            )
            .unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text.as_ref(), "没有匹配的设备");
        assert!(!lines[0].wrap_boundaries().is_empty());
        let restored = lines
            .iter()
            .map(|line| line.text.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(restored, text);
    });
}
