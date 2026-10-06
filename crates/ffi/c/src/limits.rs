//! Sizes C callers allocate for.

/// The largest ticket Kaisatsu accepts, in bytes. Size ticket buffers with it.
pub const KAISATSU_MAX_TICKET_LEN: usize = 2048;
const _: () = assert!(KAISATSU_MAX_TICKET_LEN == kaisatsu::wire::MAX_TICKET_LEN);
