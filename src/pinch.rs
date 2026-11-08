//! Trackpad pinch → timeline zoom.
//!
//! gpui 0.2.2 drops macOS magnify events (a two-finger pinch), so a pinch
//! never reached the timeline. An AppKit local event monitor catches them
//! and sends each step's magnification (≈ ±0.01–0.1, `1 + m` is the scale
//! change) to the editor, which zooms around the pointer like ⌘ + wheel.
//! Off macOS the receiver simply never gets anything.

/// Starts the monitor once; returns the stream of magnification steps.
pub fn listen() -> async_channel::Receiver<f32> {
    let (tx, rx) = async_channel::unbounded();
    #[cfg(target_os = "macos")]
    mac::install(tx);
    #[cfg(not(target_os = "macos"))]
    drop(tx);
    rx
}

#[cfg(target_os = "macos")]
// objc 0.2's macros test `feature = "cargo-clippy"`.
#[allow(unexpected_cfgs)]
mod mac {
    use block::ConcreteBlock;
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    /// `NSEventMaskMagnify` (1 << `NSEventTypeMagnify`, 30).
    const MASK_MAGNIFY: u64 = 1 << 30;

    pub fn install(tx: async_channel::Sender<f32>) {
        let handler = ConcreteBlock::new(move |event: *mut Object| -> *mut Object {
            if !event.is_null() {
                let m: f64 = unsafe { msg_send![event, magnification] };
                if m.is_finite() && m != 0. {
                    let _ = tx.try_send(m as f32);
                }
            }
            event
        })
        .copy();
        unsafe {
            // AppKit keeps the (copied) block; the monitor lives as long as
            // the app, so neither is ever removed.
            let monitor: *mut Object = msg_send![
                class!(NSEvent),
                addLocalMonitorForEventsMatchingMask: MASK_MAGNIFY
                handler: &*handler
            ];
            let _: *mut Object = msg_send![monitor, retain];
        }
        std::mem::forget(handler);
    }
}
