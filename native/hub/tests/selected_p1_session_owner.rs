//! 独立 W1-S test binary，复用已闭合 W1-L 和其固定逐笔 allocator recorder。

#[path = "selected_p1_foundation.rs"]
mod foundation;
pub use foundation::selected_p1;
pub use foundation::{audit, capacity, certificate, layout, ledger, owner, secret};

#[path = "../src/selected_p1/session_owner/mod.rs"]
pub mod session_owner;
