//! 实际 codec H；不是 std/PTY/线程/helper 全集的生产进程证书。

use crate::selected_p1::model::layout::NumericReject;
use crate::selected_p1::resource::ledger::{
    Admission, Amount, ProcessLedger, ReservationReject, ResourceClass,
};
use crate::selected_p1::resource::owner::{OwnerLayout, SharedOwner};

use super::launch::{Counter, Sink, Writer, config, with_args};
use super::{
    ARG_CAPACITY, BorrowedSystem, DesktopPlatform, ENV_CAPACITY, EnvClearExplicitBuilder,
    ExeIdentity, LaunchPurpose, ScopeReject, SourceBinding, windows_crt_argument,
};

#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub struct Span {
    pub offset: usize,
    pub len: usize,
}

#[derive(Clone, Copy)]
pub struct EnvSpan {
    pub key: &'static str,
    pub value: Span,
}
const EMPTY_ENV: EnvSpan = EnvSpan {
    key: "",
    value: Span { offset: 0, len: 0 },
};

pub struct LaunchRecord {
    pub binding: SourceBinding,
    pub purpose: LaunchPurpose,
    pub config: Span,
    pub program: Span,
    pub cwd: Span,
    pub args: [Span; ARG_CAPACITY],
    pub arg_count: usize,
    pub env: [EnvSpan; ENV_CAPACITY],
    pub env_count: usize,
    pub windows_application: Span,
    pub windows_cwd: Span,
    pub windows_command: Span,
    pub windows_environment: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutsideRustCost {
    OsAndOpenSshNotBoundedHere,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutReadiness {
    CodecActualProcessConditional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CostModel {
    pub backing: OwnerLayout,
    pub payload_bytes: usize,
    pub config_bytes: usize,
    pub windows_command_units: usize,
    pub windows_environment_units: usize,
    pub readiness: LayoutReadiness,
    pub outside_rust: OutsideRustCost,
}

/// 只借用捕获值；没有 to_string、路径 clone、argv Vec 或环境 map。
pub struct BorrowedShape<'a> {
    input: BorrowedSystem<'a>,
    image: &'a ExeIdentity<'a>,
    purpose: LaunchPurpose,
    model: CostModel,
}

fn add(a: usize, b: usize) -> Result<usize, ScopeReject> {
    a.checked_add(b).ok_or(ScopeReject::Encoding)
}
fn wide_len(value: &str) -> Result<usize, ScopeReject> {
    add(value.encode_utf16().count(), 1)
}

fn wide_command(
    program: &str,
    args: &[&str],
    mut emit: impl FnMut(u16) -> Result<(), ScopeReject>,
) -> Result<(), ScopeReject> {
    windows_crt_argument(program, &mut emit)?;
    for arg in args {
        emit(b' ' as u16)?;
        windows_crt_argument(arg, &mut emit)?;
    }
    emit(0)
}

fn wide_environment(
    env: &EnvClearExplicitBuilder<'_>,
    mut emit: impl FnMut(u16) -> Result<(), ScopeReject>,
) -> Result<(), ScopeReject> {
    // Windows 要求不区分大小写的有序块；固定小集合在栈上排序，无中间分配。
    let mut entries = [None; ENV_CAPACITY];
    let mut len = 0;
    for entry in env.entries() {
        entries[len] = Some(entry);
        len += 1;
    }
    // keys 均固定，大写除 SystemRoot；按 ASCII 折叠比较不分配。
    entries[..len].sort_unstable_by(|a, b| {
        a.map(|v| v.key.bytes().map(|c| c.to_ascii_uppercase()))
            .into_iter()
            .flatten()
            .cmp(
                b.map(|v| v.key.bytes().map(|c| c.to_ascii_uppercase()))
                    .into_iter()
                    .flatten(),
            )
    });
    for entry in entries[..len].iter().flatten() {
        for unit in entry.key.encode_utf16() {
            emit(unit)?;
        }
        emit(b'=' as u16)?;
        for unit in entry.value.encode_utf16() {
            emit(unit)?;
        }
        emit(0)?;
    }
    emit(0)
}

impl<'a> BorrowedShape<'a> {
    pub fn checked(
        input: BorrowedSystem<'a>,
        image: &'a ExeIdentity<'a>,
        purpose: LaunchPurpose,
    ) -> Result<Self, ScopeReject> {
        input.validate()?;
        image.revalidate()?;
        let env = EnvClearExplicitBuilder::for_system(input, purpose)?;
        let mut counter = Counter(0);
        config(&mut counter, input)?;
        let config_bytes = counter.0;
        counter.put(image.path())?;
        counter.put(input.paths.cwd)?;
        with_args(input, purpose, |args| -> Result<(), ScopeReject> {
            for arg in args {
                counter.put(arg)?;
            }
            Ok(())
        })?;
        for entry in env.entries() {
            counter.put(entry.value)?;
        }
        let utf8_bytes = counter.0;
        let mut command_units = 0;
        let mut environment_units = 0;
        let payload_bytes = if input.platform == DesktopPlatform::Windows {
            with_args(input, purpose, |args| {
                wide_command(image.path(), args, |_| {
                    command_units = add(command_units, 1)?;
                    Ok(())
                })
            })?;
            wide_environment(&env, |_| {
                environment_units = add(environment_units, 1)?;
                Ok(())
            })?;
            if command_units > 32_767 || environment_units > 32_767 {
                return Err(ScopeReject::Encoding);
            }
            let wide_units = add(
                add(wide_len(image.path())?, wide_len(input.paths.cwd)?)?,
                add(command_units, environment_units)?,
            )?;
            add(
                add(utf8_bytes, utf8_bytes % 2)?,
                wide_units.checked_mul(2).ok_or(ScopeReject::Encoding)?,
            )?
        } else {
            utf8_bytes
        };
        let model = CostModel {
            backing: OwnerLayout::actual::<LaunchRecord>(payload_bytes)?,
            payload_bytes,
            config_bytes,
            windows_command_units: command_units,
            windows_environment_units: environment_units,
            readiness: LayoutReadiness::CodecActualProcessConditional,
            outside_rust: OutsideRustCost::OsAndOpenSshNotBoundedHere,
        };
        Ok(Self {
            input,
            image,
            purpose,
            model,
        })
    }

    pub fn cost_model(&self) -> CostModel {
        self.model
    }

    pub fn admit(self, ledger: &'a ProcessLedger) -> Result<OwnedLaunch<'a>, ScopeReject> {
        let amount = Amount {
            rust_bytes: self.model.backing.backing.size(),
            c_bytes: 0,
            slots: 1,
        };
        let grant = ledger
            .try_reserve_detailed(
                amount,
                ResourceClass::Source,
                Admission {
                    epoch: self.input.binding.source.epoch,
                    source: Some(self.input.binding.source),
                },
            )
            .map_err(|error| match error {
                ReservationReject::Numeric(error) => ScopeReject::Resource(error),
                ReservationReject::ByteLimit {
                    requested,
                    current,
                    limit,
                } => ScopeReject::Quota {
                    requested,
                    current,
                    limit,
                },
                ReservationReject::ClassSlots { .. } => ScopeReject::Slots,
            })?;
        let owner = SharedOwner::try_init(grant, self.model.payload_bytes, 0, |bytes| {
            self.encode(bytes)
                .map_err(|_| NumericReject::ConstructionFailure)
        })?;
        Ok(OwnedLaunch { owner })
    }

    fn encode(&self, bytes: &mut [u8]) -> Result<LaunchRecord, ScopeReject> {
        let mut writer = Writer { bytes, used: 0 };
        config(&mut writer, self.input)?;
        let config_span = Span {
            offset: 0,
            len: writer.used,
        };
        fn put(writer: &mut Writer<'_>, value: &str) -> Result<Span, ScopeReject> {
            let offset = writer.used;
            writer.put(value)?;
            Ok(Span {
                offset,
                len: value.len(),
            })
        }
        let program = put(&mut writer, self.image.path())?;
        let cwd = put(&mut writer, self.input.paths.cwd)?;
        let mut args = [Span::default(); ARG_CAPACITY];
        let mut arg_count = 0;
        with_args(
            self.input,
            self.purpose,
            |values| -> Result<(), ScopeReject> {
                for arg in values {
                    args[arg_count] = put(&mut writer, arg)?;
                    arg_count += 1;
                }
                Ok(())
            },
        )?;
        let mut env = [EMPTY_ENV; ENV_CAPACITY];
        let mut env_count = 0;
        let builder = EnvClearExplicitBuilder::for_system(self.input, self.purpose)?;
        for entry in builder.entries() {
            env[env_count] = EnvSpan {
                key: entry.key,
                value: put(&mut writer, entry.value)?,
            };
            env_count += 1;
        }
        if self.input.platform == DesktopPlatform::Windows && !writer.used.is_multiple_of(2) {
            writer.put("\0")?;
        }
        fn wide(
            writer: &mut Writer<'_>,
            operation: impl FnOnce(
                &mut dyn FnMut(u16) -> Result<(), ScopeReject>,
            ) -> Result<(), ScopeReject>,
        ) -> Result<Span, ScopeReject> {
            let start = writer.used;
            operation(&mut |unit| {
                let end = add(writer.used, 2)?;
                writer
                    .bytes
                    .get_mut(writer.used..end)
                    .ok_or(ScopeReject::Encoding)?
                    .copy_from_slice(&unit.to_ne_bytes());
                writer.used = end;
                Ok(())
            })?;
            Ok(Span {
                offset: start,
                len: writer.used - start,
            })
        }
        fn wide_string(
            value: &str,
            emit: &mut dyn FnMut(u16) -> Result<(), ScopeReject>,
        ) -> Result<(), ScopeReject> {
            for unit in value.encode_utf16() {
                emit(unit)?;
            }
            emit(0)
        }
        let (windows_application, windows_cwd, windows_command, windows_environment) =
            if self.input.platform == DesktopPlatform::Windows {
                (
                    wide(&mut writer, |emit| wide_string(self.image.path(), emit))?,
                    wide(&mut writer, |emit| wide_string(self.input.paths.cwd, emit))?,
                    wide(&mut writer, |emit| {
                        with_args(self.input, self.purpose, |args| {
                            wide_command(self.image.path(), args, emit)
                        })
                    })?,
                    wide(&mut writer, |emit| wide_environment(&builder, emit))?,
                )
            } else {
                (
                    Span::default(),
                    Span::default(),
                    Span::default(),
                    Span::default(),
                )
            };
        if writer.used != self.model.payload_bytes {
            return Err(ScopeReject::Encoding);
        }
        Ok(LaunchRecord {
            binding: self.input.binding,
            purpose: self.purpose,
            config: config_span,
            program,
            cwd,
            args,
            arg_count,
            env,
            env_count,
            windows_application,
            windows_cwd,
            windows_command,
            windows_environment,
        })
    }
}

pub struct OwnedLaunch<'root> {
    owner: SharedOwner<'root, LaunchRecord>,
}

impl<'root> OwnedLaunch<'root> {
    pub fn record(&self) -> &LaunchRecord {
        self.owner.value()
    }
    fn text(&self, span: Span) -> &str {
        // 范围仅来自本模块的 initialized UTF-8 编码；错误是内部不变量破坏，不能静默启动。
        match std::str::from_utf8(&self.owner.payload()[span.offset..span.offset + span.len]) {
            Ok(value) => value,
            Err(_) => unreachable!("System codec UTF-8 不变量"),
        }
    }
    pub fn program(&self) -> &str {
        self.text(self.record().program)
    }
    pub fn cwd(&self) -> &str {
        self.text(self.record().cwd)
    }
    pub fn config(&self) -> &str {
        self.text(self.record().config)
    }
    pub fn args(&self) -> impl Iterator<Item = &str> {
        self.record().args[..self.record().arg_count]
            .iter()
            .map(|span| self.text(*span))
    }
    pub fn environment(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.record().env[..self.record().env_count]
            .iter()
            .map(|entry| (entry.key, self.text(entry.value)))
    }
    pub fn windows_units(&self, span: Span) -> Result<&[u16], ScopeReject> {
        let bytes = self
            .owner
            .payload()
            .get(span.offset..add(span.offset, span.len)?)
            .ok_or(ScopeReject::Encoding)?;
        if !span.offset.is_multiple_of(2)
            || !span.len.is_multiple_of(2)
            || !(bytes.as_ptr() as usize).is_multiple_of(2)
        {
            return Err(ScopeReject::Encoding);
        }
        // SAFETY: backing 对齐 >= 2，已验证范围/对齐；u16 所有位模式均有效，持有 immutable owner。
        Ok(unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len() / 2) })
    }
    pub fn held_bytes(&self) -> usize {
        self.owner.admitted().rust_bytes
    }
    /// caller 必须让进程、helper、pipe、offline/late root 各持真实 lease 至最后使用。
    /// 这里只保留 backing；不声称 PID 退出已完成 wait/drain/join。
    pub fn retain(&self) -> Result<Self, ScopeReject> {
        Ok(Self {
            owner: self.owner.try_clone()?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActualProcessLayout {
    pub rust_before_spawn_upper: usize,
    pub c_before_spawn_upper: usize,
    pub cleanup_rust_upper: usize,
    pub cleanup_c_upper: usize,
    pub workers: u32,
}

/// 只读实际全集源证明接口；不能用 Native/noNetwork/四 helper 常量填充。
///
/// # Safety
/// 实现者须覆盖实际 compiler/target/库 feature、stdlib/PTY/IPC/worker/所有 helper、
/// 首份秘密至 late root 的 backing；给出完整分配前上界与取消/最后释放路径。
/// 数字来源必须是可复核源界而不是峰值、任意 allowance 或 stub_spawn。
pub unsafe trait ActualProcessSourceClosure {
    fn matches(&self, image: &ExeIdentity<'_>, binding: SourceBinding) -> bool;
    fn actual_layout(&self) -> ActualProcessLayout;
}

pub struct BeforeSpawnGrant<'root> {
    owner: SharedOwner<'root, ProcessReservation>,
}

pub struct ProcessReservation {
    pub binding: SourceBinding,
    pub actual_layout: ActualProcessLayout,
}

pub fn admit_actual_process<'root>(
    ledger: &'root ProcessLedger,
    image: &ExeIdentity<'_>,
    binding: SourceBinding,
    source: &dyn ActualProcessSourceClosure,
) -> Result<BeforeSpawnGrant<'root>, ScopeReject> {
    image.revalidate()?;
    if !source.matches(image, binding) {
        return Err(ScopeReject::SourceClosureRequired);
    }
    let layout = source.actual_layout();
    let runtime_upper = add(layout.rust_before_spawn_upper, layout.cleanup_rust_upper)?;
    let owner_layout = OwnerLayout::actual::<ProcessReservation>(0)?;
    let rust_bytes = owner_layout.required_rust(runtime_upper)?;
    let c_bytes = add(layout.c_before_spawn_upper, layout.cleanup_c_upper)?;
    let permit = ledger
        .try_reserve_detailed(
            Amount {
                rust_bytes,
                c_bytes,
                slots: layout.workers,
            },
            ResourceClass::Runtime,
            Admission {
                epoch: binding.source.epoch,
                source: Some(binding.source),
            },
        )
        .map_err(|error| match error {
            ReservationReject::Numeric(error) => ScopeReject::Resource(error),
            ReservationReject::ByteLimit {
                requested,
                current,
                limit,
            } => ScopeReject::Quota {
                requested,
                current,
                limit,
            },
            ReservationReject::ClassSlots { .. } => ScopeReject::Slots,
        })?;
    let owner = SharedOwner::try_init(permit, 0, runtime_upper, |_| {
        Ok(ProcessReservation {
            binding,
            actual_layout: layout,
        })
    })?;
    Ok(BeforeSpawnGrant { owner })
}

impl<'root> BeforeSpawnGrant<'root> {
    pub fn held(&self) -> Amount {
        self.owner.admitted()
    }
    pub fn reservation(&self) -> &ProcessReservation {
        self.owner.value()
    }
    pub fn retain(&self) -> Result<BeforeSpawnGrant<'root>, ScopeReject> {
        Ok(BeforeSpawnGrant {
            owner: self.owner.try_clone()?,
        })
    }
}
