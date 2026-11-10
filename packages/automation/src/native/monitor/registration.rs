//! Confirm the OS accepted the complete mask: CGEventTapCreate may silently
//! clear denied event bits. A non-NULL tap alone does not prove keyboard access.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Info {
    id: u32,
    point: u32,
    options: u32,
    mask: u64,
    process: i32,
    target_process: i32,
    enabled: bool,
    min_latency: f32,
    avg_latency: f32,
    max_latency: f32,
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetEventTapList(max: u32, list: *mut Info, count: *mut u32) -> i32;
}
pub fn snapshot() -> Result<Vec<Info>, &'static str> {
    let mut list = vec![Info::default(); 1024];
    let mut count = 0;
    if unsafe { CGGetEventTapList(list.len() as u32, list.as_mut_ptr(), &mut count) } != 0
        || count as usize >= list.len()
    {
        return Err("tap_inventory_unavailable");
    }
    list.truncate(count as usize);
    list.retain(|tap| tap.process == std::process::id() as i32);
    Ok(list)
}
pub fn verify(before: &[Info], after: &[Info], mask: u64) -> Result<(), &'static str> {
    let mut created = after.iter().filter(|tap| {
        tap.point == 1
            && tap.options == 1
            && tap.target_process == 0
            && !before.iter().any(|previous| previous.id == tap.id)
    });
    let tap = created.next().ok_or("tap_identity_ambiguous")?;
    if created.next().is_some() {
        return Err("tap_identity_ambiguous");
    }
    if !tap.enabled || tap.mask & mask != mask {
        return Err("tap_mask_incomplete");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_event_masks_are_not_advertised_as_ready() {
        let mouse_only = Info {
            id: 1,
            point: 1,
            options: 1,
            mask: 1 << 5,
            enabled: true,
            ..Default::default()
        };
        let required = (1 << 5) | (1 << 10) | (1 << 11) | (1 << 12);
        assert_eq!(
            verify(&[], &[mouse_only], required),
            Err("tap_mask_incomplete")
        );
        let complete = Info {
            mask: required,
            ..mouse_only
        };
        assert!(verify(&[], &[complete], required).is_ok());
        // A pre-existing full tap must not mask this listener's missing keys.
        assert_eq!(
            verify(
                &[complete],
                &[
                    complete,
                    Info {
                        id: 2,
                        ..mouse_only
                    }
                ],
                required
            ),
            Err("tap_mask_incomplete")
        );
    }
    #[test]
    fn concurrent_ambiguous_registration_or_disabled_tap_is_rejected() {
        let tap = Info {
            id: 1,
            point: 1,
            options: 1,
            mask: 1,
            enabled: true,
            ..Default::default()
        };
        assert_eq!(
            verify(&[], &[tap, Info { id: 2, ..tap }], 1),
            Err("tap_identity_ambiguous")
        );
        assert_eq!(
            verify(
                &[],
                &[Info {
                    enabled: false,
                    ..tap
                }],
                1
            ),
            Err("tap_mask_incomplete")
        );
    }
}
