//! Capability-based presentation selection. See WAIFU-PATCH.md.

pub(super) fn resolve_present_mode(
    present_mode: wgpu::PresentMode,
    available: &[wgpu::PresentMode],
) -> wgpu::PresentMode {
    let fallbacks = match present_mode {
        wgpu::PresentMode::AutoVsync => {
            &[wgpu::PresentMode::FifoRelaxed, wgpu::PresentMode::Fifo][..]
        }
        wgpu::PresentMode::AutoNoVsync => &[
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Fifo,
        ][..],
        // Waifu Studio: preserve VSync if Mailbox is unavailable.
        wgpu::PresentMode::Mailbox => &[wgpu::PresentMode::Mailbox, wgpu::PresentMode::Fifo][..],
        // Always end in FIFO to make sure it's always supported
        x => &[x, wgpu::PresentMode::Fifo][..],
    };
    fallbacks
        .iter()
        .copied()
        .find(|fallback| available.contains(fallback))
        .unwrap_or_else(|| {
            unreachable!(
                "Fallback system failed to choose present mode. \
                            This is a bug. Mode: {:?}, Options: {:?}",
                present_mode, available
            );
        })
}

#[cfg(test)]
mod presentation_policy_tests {
    use super::resolve_present_mode;
    use wgpu::PresentMode::*;

    #[test]
    fn mailbox_is_preferred_when_supported_regardless_of_capability_order() {
        for supported in [vec![Fifo, Immediate, Mailbox], vec![Mailbox, Fifo]] {
            assert_eq!(resolve_present_mode(Mailbox, &supported), Mailbox);
        }
    }

    #[test]
    fn unavailable_mailbox_preserves_vsync_even_when_immediate_is_available() {
        assert_eq!(resolve_present_mode(Mailbox, &[Immediate, Fifo]), Fifo);
        assert_eq!(resolve_present_mode(Mailbox, &[Fifo]), Fifo);
    }

    #[test]
    fn explicit_fifo_is_not_promoted_to_mailbox() {
        assert_eq!(
            resolve_present_mode(Fifo, &[Immediate, Mailbox, Fifo]),
            Fifo
        );
    }

    #[test]
    fn existing_explicit_no_vsync_and_auto_policies_are_preserved() {
        assert_eq!(
            resolve_present_mode(Immediate, &[Immediate, Fifo]),
            Immediate
        );
        assert_eq!(resolve_present_mode(Immediate, &[Fifo]), Fifo);
        assert_eq!(
            resolve_present_mode(AutoNoVsync, &[Mailbox, Immediate, Fifo]),
            Immediate
        );
        assert_eq!(resolve_present_mode(AutoNoVsync, &[Mailbox, Fifo]), Mailbox);
        assert_eq!(
            resolve_present_mode(AutoVsync, &[Mailbox, FifoRelaxed, Fifo]),
            FifoRelaxed
        );
        assert_eq!(resolve_present_mode(AutoVsync, &[Fifo]), Fifo);
    }
}
