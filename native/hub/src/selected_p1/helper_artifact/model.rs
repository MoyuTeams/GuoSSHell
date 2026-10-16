//! 应用独立解释 exact MSVC 源模型；不链接或复制 helper 私有 Rust 模块。
//!
//! 来源是受 recipe 摘要绑定的 rust_budget/{source,capacity,launch,mod}.rs 与
//! normal_v2.rs：args/windows::parse_lp_cmd_line、wtf8::from_wide、CString、
//! stdin Buffer 12288、stdout LineWriter 1024。控制字段在栈/static 内，不重复计费。
//! runtime current_id/native TLS、numeric IO/Report、cleanup 无新增 Rust heap；
//! UUID const parser 与写入固定数组的格式化也无 heap。三路真实 pipe 排除 console
//! String::from_utf16；test/env/线程/动态诊断分支不属于 normal 闭包。

use super::Reject;
use crate::selected_p1::helper_runtime::process::{LaunchIdentity, LaunchShape};
use std::{alloc::Layout, ffi::OsStr, os::windows::ffi::OsStrExt};

pub const COMPILER: &str = "b940084d7eb6a299eb4bfeb8e34901bc051e7ac4";
pub const TARGET: &str = "x86_64-pc-windows-msvc";
pub const FEATURES: &str = "rusqlite=0.37.0:bundled,modern_sqlite;libsqlite3-sys=0.35.0:bundled,bundled_bindings,cc,default,min_sqlite_version_3_14_0,pkg-config,vcpkg;uuid=1.24.0:default,rng,serde,std,v4";
pub(super) const TARGET_CFG: &str = concat!(
    "debug_assertions\npanic=\"unwind\"\ntarget_abi=\"\"\ntarget_arch=\"x86_64\"\ntarget_endian=\"little\"\ntarget_env=\"msvc\"\ntarget_family=\"windows\"\n",
    "target_feature=\"cmpxchg16b\"\ntarget_feature=\"fxsr\"\ntarget_feature=\"sse\"\ntarget_feature=\"sse2\"\ntarget_feature=\"sse3\"\n",
    "target_has_atomic=\"128\"\ntarget_has_atomic=\"16\"\ntarget_has_atomic=\"32\"\ntarget_has_atomic=\"64\"\ntarget_has_atomic=\"8\"\ntarget_has_atomic=\"ptr\"\n",
    "target_has_atomic_primitive_alignment=\"128\"\ntarget_has_atomic_primitive_alignment=\"16\"\ntarget_has_atomic_primitive_alignment=\"32\"\ntarget_has_atomic_primitive_alignment=\"64\"\ntarget_has_atomic_primitive_alignment=\"8\"\ntarget_has_atomic_primitive_alignment=\"ptr\"\n",
    "target_os=\"windows\"\ntarget_pointer_width=\"64\"\ntarget_vendor=\"pc\"\nwindows",
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Costs {
    pub argv: usize,
    pub database_c_string: usize,
    pub stdio_globals: usize,
    pub sql_upper: usize,
}
impl Costs {
    pub fn total(self) -> Result<usize, Reject> {
        [
            self.argv,
            self.database_c_string,
            self.stdio_globals,
            self.sql_upper,
        ]
        .into_iter()
        .try_fold(0, add)
    }
}

pub(super) fn add(a: usize, b: usize) -> Result<usize, Reject> {
    a.checked_add(b).ok_or(Reject::Overflow)
}
fn array(size: usize, align: usize, count: usize) -> Result<usize, Reject> {
    Layout::from_size_align(size.checked_mul(count).ok_or(Reject::Overflow)?, align)
        .map(|layout| layout.size())
        .map_err(|_| Reject::Overflow)
}
fn peak(size: usize, align: usize, initial: usize, len: usize) -> Result<usize, Reject> {
    if len <= initial {
        return array(size, align, initial);
    }
    let minimum = if size == 1 { 8 } else { 4 };
    let old = len - 1;
    let next = old
        .checked_mul(2)
        .ok_or(Reject::Overflow)?
        .max(len)
        .max(minimum);
    Ok(array(size, align, initial)?.max(add(array(size, align, old)?, array(size, align, next)?)?))
}

pub(super) fn bound(identity: LaunchIdentity<'_>, shape: &LaunchShape) -> Result<Costs, Reject> {
    if identity.arguments.len() != 1
        || shape.arguments != 1
        || !shape.inherits_environment
        || !shape.inherits_cwd
        || identity.executable.encode_wide().any(|u| u == 0 || u == 34)
        || !std::path::Path::new(identity.executable).is_absolute()
        || identity.arguments[0].to_str().is_none()
        || identity.arguments[0].encode_wide().any(|u| u == 0)
    {
        return Err(Reject::Launch);
    }
    let exe = identity.executable.encode_wide().count();
    let db = identity.arguments[0].encode_wide().count();
    let mut command_units = 1;
    for path in [identity.executable, identity.arguments[0]] {
        command_units = add(command_units, quoted_units(path)?)?;
    }
    if shape.executable_units != exe
        || shape.argument_units[0] != db
        || shape.argument_units[1..] != [0; 7]
        || shape.command_units != command_units
    {
        return Err(Reject::Launch);
    }
    // actual MSVC OsString/Wtf8Buf 32/8；每个 cur Vec<u16> 与旧新 WTF8 backing
    // 保守并列相加，ret Vec 的两个元素采用 grow_amortized(minimum=4) 界。
    let mut argv = peak(32, 8, 0, 2)?;
    for path in [identity.executable, identity.arguments[0]] {
        let units = path.encode_wide().count();
        argv = add(argv, peak(2, 2, 0, units)?)?;
        argv = add(argv, peak(1, 1, units, path.as_encoded_bytes().len())?)?;
    }
    Ok(Costs {
        argv,
        database_c_string: array(
            1,
            1,
            add(identity.arguments[0].as_encoded_bytes().len(), 1)?,
        )?,
        stdio_globals: add(array(1, 1, 12288)?, array(1, 1, 1024)?)?,
        // 已核对 source::legacy_sql_upper 的列集合/format/collect/join/CString 界。
        // normal FixedCStr 无这些 heap；保留此获准保守项而不缩减 H。
        sql_upper: 8512,
    })
}
/// 与 Windows 同步 launcher 相同的纯借用形状，用于构造 request 的 H 输入。
pub fn launch_shape(identity: LaunchIdentity<'_>) -> Result<LaunchShape, Reject> {
    if identity.arguments.len() != 1 {
        return Err(Reject::Launch);
    }
    let mut argument_units = [0; 8];
    argument_units[0] = identity.arguments[0].encode_wide().count();
    Ok(LaunchShape {
        argument_units,
        arguments: 1,
        executable_units: identity.executable.encode_wide().count(),
        command_units: add(
            add(
                quoted_units(identity.executable)?,
                quoted_units(identity.arguments[0])?,
            )?,
            1,
        )?,
        inherits_environment: true,
        inherits_cwd: true,
    })
}
fn quoted_units(path: &OsStr) -> Result<usize, Reject> {
    let mut count = 2;
    let mut slashes = 0;
    for unit in path.encode_wide() {
        if unit == 92 {
            slashes = add(slashes, 1)?;
        } else {
            count = add(
                count,
                if unit == 34 {
                    add(slashes.checked_mul(2).ok_or(Reject::Overflow)?, 2)?
                } else {
                    add(slashes, 1)?
                },
            )?;
            slashes = 0;
        }
    }
    add(count, slashes.checked_mul(2).ok_or(Reject::Overflow)?)
}
