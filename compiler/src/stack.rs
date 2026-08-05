//! Stack-growth helper for the recursive parser and renderer.
//!
//! Recursive descent over deeply nested input can exhaust a thread's stack. The
//! `MAX_DEPTH` / `MAX_RENDER_DEPTH` caps bound how deep we go, but the renderer's
//! and element parser's stack frames are large enough that even a modest cap can
//! approach a 2 MB worker-thread stack. On native targets we grow the stack on
//! demand (as rustc itself does) so the cap - not the raw stack size - is the
//! real limit. On wasm there is no way to grow the stack, so this is a no-op and
//! the depth cap alone keeps it safe.

/// Grow when fewer than this many bytes of stack remain.
const RED_ZONE: usize = 128 * 1024;
/// Size of each freshly allocated stack segment.
const STACK_SIZE: usize = 2 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(RED_ZONE, STACK_SIZE, f)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn with_stack<R>(f: impl FnOnce() -> R) -> R {
    f()
}
