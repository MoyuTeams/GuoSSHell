//! W1-S 单操作接口；父接线前不启用 normal execute。
//! 完整 Snapshot/Source/CloneShape 的来源证明由各自所有者提供。

pub mod context;
pub mod gates;
pub mod inline;
pub mod native;
pub mod operation;
pub mod session;

#[cfg(test)]
mod tests;
