#![cfg(feature = "cg")]

use apple_cf::cg::CGContext;

#[test]
fn bitmap_context_storage_is_initialized() {
    let context = CGContext::new_rgba8(4, 4).expect("context");
    // SAFETY: the context has no aliases and no drawing occurs while the slice lives.
    let bytes = unsafe { context.as_bytes() };
    assert!(bytes.iter().all(|byte| *byte == 0));
}

#[test]
fn context_clones_share_drawing_state() {
    let context = CGContext::new_rgba8(4, 4).expect("context");
    let alias = context.clone();
    alias.set_rgb_fill_color(1.0, 0.0, 0.0, 1.0);
    alias.fill_rect(0.0, 0.0, 4.0, 4.0);
    drop(alias);

    // SAFETY: drawing is complete and the retained alias has been dropped.
    let bytes = unsafe { context.as_bytes() };
    assert_eq!(&bytes[..4], &[255, 0, 0, 255]);
}
