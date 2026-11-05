//! One module per page of the window. Each is an `impl SeexUi` block so the
//! pages can live in their own files while sharing the single view entity that
//! GPUI renders.

pub mod export;
pub mod font;
pub mod libraries;
pub mod monitor;
pub mod more;
pub mod pads;
