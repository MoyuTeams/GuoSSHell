//! 正常候选的应用侧受信构建记录；不注册 Prepare、actor 或 VM。
//! record/recipe 来自父受控构建输出；digest 绑定本次审阅的 source 闭包，
//! 实际映像以不共享写入/删除的 handle 持有，spawn 再独立校验 SHA。

mod json;
pub mod model;
mod selected;

pub use selected::{Fields, decode_loaded, project_loaded};

use crate::selected_p1::{
    helper_runtime::process::{
        CertifiedHelper, Failure, HelperBuildRecord, LaunchIdentity, LaunchShape, Request,
    },
    model::layout::NumericReject,
    resource::{
        ledger::{Admission, Amount, ProcessLedger, ResourceClass},
        owner::{OwnerLayout, SharedOwner},
    },
};
use json::Value;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::Read,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ, OPEN_EXISTING,
    },
};

pub const REQUEST_MAGIC: &[u8; 8] = b"SHSELREQ";
pub const ARTIFACT_SHA256: &str =
    "d7542ca44ac1504c4a9c6fc25ce77680a52ebf9eedd559b2c7deef8dabc2a774";
const RECIPE_SHA256: &str = "1b887c73c6c629a8197439738bd4eed601bc3b1b5f5500ac6cc4919cd630c3a1";
const RECORD_MAX: usize = 32768;
const RECIPE_MAX: usize = 32768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Record,
    Recipe,
    Compiler,
    Target,
    Cfg,
    Features,
    Source,
    Artifact,
    Launch,
    Request,
    Runtime,
    Overflow,
    Io,
    Budget,
    Allocation,
    Admission(NumericReject),
}
impl From<NumericReject> for Reject {
    fn from(value: NumericReject) -> Self {
        match value {
            NumericReject::AllocationFailure => Self::Allocation,
            NumericReject::RustBudgetExceeded => Self::Budget,
            NumericReject::EpochMismatch
            | NumericReject::SourceMismatch
            | NumericReject::InvalidSource => Self::Admission(value),
            _ => Self::Overflow,
        }
    }
}
impl Reject {
    fn runtime(self) -> Failure {
        match self {
            Self::Budget => Failure::Budget,
            Self::Allocation => Failure::Allocation,
            Self::Io => Failure::Io,
            Self::Request => Failure::Protocol,
            _ => Failure::Unsupported,
        }
    }
}

/// 路径均由父拥有：构建输出、对应源码校验树及已捕获的 catalog DB，不发现用户路径。
/// 校验树可为受控的相同源闭包副本；recipe 中的原构建根不改写，逐文件摘要必须相同。
pub struct Inputs<'a> {
    pub record: &'a Path,
    pub recipe: &'a Path,
    pub source_root: &'a Path,
    pub executable: &'a OsStr,
    pub database: &'a OsStr,
}

#[repr(C)]
pub struct RecordBody {
    record_len: usize,
    recipe_len: usize,
    executable_len: usize,
    database_len: usize,
    record_sha256: [u8; 32],
    artifact_sha256: [u8; 32],
    // 映像文件的 write/delete 排他性保到最后 provider holder 析构。
    artifact: File,
}

pub struct OwnedRecord<'r> {
    owner: SharedOwner<'r, RecordBody>,
}
impl<'r> OwnedRecord<'r> {
    /// controlled record 只接受本模型支持的生成式 schema 与受审 recipe。
    /// 无 unbounded deserialize；实际 tail、path copy 和 header 均在 grant 之后。
    pub fn load(
        inputs: Inputs<'_>,
        root: &'r ProcessLedger,
        admission: Admission,
    ) -> Result<Self, Reject> {
        if !inputs.source_root.is_absolute()
            || !Path::new(inputs.executable).is_absolute()
            || !Path::new(inputs.database).is_absolute()
            || inputs.executable.encode_wide().any(|u| u == 0 || u == 34)
            || inputs.database.to_str().is_none()
            || inputs.database.encode_wide().any(|u| u == 0)
        {
            return Err(Reject::Launch);
        }
        let mut record = open(inputs.record.as_os_str(), None)?;
        let mut recipe = open(inputs.recipe.as_os_str(), None)?;
        let artifact = open(inputs.executable, None)?;
        let record_len = file_len(&record, RECORD_MAX)?;
        let recipe_len = file_len(&recipe, RECIPE_MAX)?;
        let executable_len = inputs.executable.as_encoded_bytes().len();
        let database_len = inputs.database.as_encoded_bytes().len();
        let tail = [record_len, recipe_len, executable_len, database_len]
            .into_iter()
            .try_fold(0, model::add)?;
        let layout = OwnerLayout::actual::<RecordBody>(tail)?;
        let permit = root.try_reserve(
            Amount {
                rust_bytes: layout.backing.size(),
                c_bytes: 0,
                slots: 0,
            },
            ResourceClass::Persistent,
            admission,
        )?;
        let mut rejection = None;
        let owner = SharedOwner::try_init(permit, tail, 0, |payload| {
            let result = (|| {
                let (record_bytes, rest) = payload.split_at_mut(record_len);
                let (recipe_bytes, rest) = rest.split_at_mut(recipe_len);
                read_exact_file(&mut record, record_bytes)?;
                read_exact_file(&mut recipe, recipe_bytes)?;
                verify_record(record_bytes, recipe_bytes, &inputs)?;
                let (actual, size) = hash_file(&artifact)?;
                let expected = hex(ARTIFACT_SHA256)?;
                if actual != expected || size != 2230272 {
                    return Err(Reject::Artifact);
                }
                let (exe_bytes, db_bytes) = rest.split_at_mut(executable_len);
                exe_bytes.copy_from_slice(inputs.executable.as_encoded_bytes());
                db_bytes.copy_from_slice(inputs.database.as_encoded_bytes());
                Ok(RecordBody {
                    record_len,
                    recipe_len,
                    executable_len,
                    database_len,
                    record_sha256: Sha256::digest(record_bytes).into(),
                    artifact_sha256: actual,
                    artifact,
                })
            })();
            result.map_err(|error| {
                rejection = Some(error);
                NumericReject::ConstructionFailure
            })
        });
        match owner {
            Ok(owner) => Ok(Self { owner }),
            Err(error) => Err(rejection.unwrap_or_else(|| error.into())),
        }
    }

    pub fn try_clone(&self) -> Result<Self, Reject> {
        Ok(Self {
            owner: self.owner.try_clone()?,
        })
    }
    pub fn admitted(&self) -> Amount {
        self.owner.admitted()
    }
    pub fn record_sha256(&self) -> [u8; 32] {
        self.owner.value().record_sha256
    }
    pub fn artifact_sha256(&self) -> [u8; 32] {
        self.owner.value().artifact_sha256
    }
    pub fn executable(&self) -> &OsStr {
        let body = self.owner.value();
        let at = body.record_len + body.recipe_len;
        // SAFETY: 精确复制同平台 OsStr 编码；UTF8/WTF8 字符边界取原 slice 边界。
        unsafe {
            OsStr::from_encoded_bytes_unchecked(&self.owner.payload()[at..at + body.executable_len])
        }
    }
    pub fn database(&self) -> &OsStr {
        let body = self.owner.value();
        let at = body.record_len + body.recipe_len + body.executable_len;
        // SAFETY: 与 executable 相同；整个 owner 保持原编码 tail 存活。
        unsafe {
            OsStr::from_encoded_bytes_unchecked(&self.owner.payload()[at..at + body.database_len])
        }
    }
    pub fn helper_costs(
        &self,
        identity: LaunchIdentity<'_>,
        shape: &LaunchShape,
    ) -> Result<model::Costs, Reject> {
        if identity.executable != self.executable() || identity.arguments != [self.database()] {
            return Err(Reject::Launch);
        }
        model::bound(identity, shape)
    }
    pub fn check(
        &self,
        identity: LaunchIdentity<'_>,
        shape: &LaunchShape,
        request: &Request,
    ) -> Result<CertifiedHelper, Reject> {
        let minimum = self.helper_costs(identity, shape)?.total()?;
        if Request::parse(request.bytes, REQUEST_MAGIC).map_err(|_| Reject::Request)? != *request {
            return Err(Reject::Request);
        }
        if request.helper_rust < minimum {
            return Err(Reject::Budget);
        }
        std::alloc::Layout::array::<u8>(request.helper_rust).map_err(|_| Reject::Overflow)?;
        std::alloc::Layout::from_size_align(request.c_max, 16).map_err(|_| Reject::Overflow)?;
        Ok(CertifiedHelper {
            amount: Amount {
                rust_bytes: request.helper_rust,
                c_bytes: request.c_max,
                slots: 1,
            },
            artifact_sha256: self.artifact_sha256(),
        })
    }
}

// SAFETY: 私有构造唯一来自 load：受审 recipe 绑定 source/lock/compiler/features/cfg，
// 支持的 normal 映像按真实 SHA/size 校验且排他持有。完整 exe/唯一 DB 参数逐值匹配，
// Shape 重放 actual Windows launcher 引用规则；SHSELREQ 请求完整 canonical 重验。
// authorize 只借用并计算源式，无 owned copy/配置读取/format。调用方实际 launcher
// 必须是 helper_runtime::windows 的环境/cwd继承及同步三路 pipe，非 console/Command。
// H 覆盖 args/WTF8/CString/stdio/error/cleanup 直到真实退出；额度由 Pool owner 保持。
unsafe impl HelperBuildRecord for OwnedRecord<'_> {
    fn request_magic(&self) -> &[u8; 8] {
        REQUEST_MAGIC
    }
    fn authorize(
        &self,
        identity: LaunchIdentity<'_>,
        shape: &LaunchShape,
        request: &Request,
    ) -> Result<CertifiedHelper, Failure> {
        self.check(identity, shape, request)
            .map_err(Reject::runtime)
    }
}

fn file_len(file: &File, maximum: usize) -> Result<usize, Reject> {
    let size = usize::try_from(file.metadata().map_err(|_| Reject::Io)?.len())
        .map_err(|_| Reject::Overflow)?;
    if size == 0 || size > maximum {
        Err(Reject::Record)
    } else {
        Ok(size)
    }
}
fn read_exact_file(file: &mut File, bytes: &mut [u8]) -> Result<(), Reject> {
    file.read_exact(bytes).map_err(|_| Reject::Io)?;
    let mut extra = [0];
    if file.read(&mut extra).map_err(|_| Reject::Io)? != 0 {
        return Err(Reject::Record);
    }
    Ok(())
}
fn open(root: &OsStr, relative: Option<&str>) -> Result<File, Reject> {
    let mut path = [0u16; 32768];
    let mut count = 0;
    let mut put = |unit| {
        if count >= path.len() - 1 || unit == 0 {
            return Err(Reject::Source);
        }
        path[count] = unit;
        count += 1;
        Ok(())
    };
    for unit in root.encode_wide() {
        put(unit)?;
    }
    if let Some(relative) = relative {
        if relative.starts_with('/')
            || relative.contains(':')
            || relative
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(Reject::Source);
        }
        put(92)?;
        for unit in relative.encode_utf16() {
            put(if unit == 47 { 92 } else { unit })?;
        }
    }
    // 原生 pathname 栈编码；不经 std 的 heap UTF16 转换或路径 join。
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(Reject::Io);
    }
    // SAFETY: handle 唯一转移 File，File Drop 只 CloseHandle。
    Ok(unsafe { File::from_raw_handle(handle) })
}
fn hash_file(mut file: &File) -> Result<([u8; 32], usize), Reject> {
    let mut hash = Sha256::new();
    let mut bytes = [0; 4096];
    let mut total = 0;
    loop {
        let len = file.read(&mut bytes).map_err(|_| Reject::Io)?;
        if len == 0 {
            break;
        }
        total = model::add(total, len)?;
        hash.update(&bytes[..len]);
    }
    Ok((hash.finalize().into(), total))
}
fn hex(text: &str) -> Result<[u8; 32], Reject> {
    if text.len() != 64 {
        return Err(Reject::Record);
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(
            text.get(index * 2..index * 2 + 2).ok_or(Reject::Record)?,
            16,
        )
        .map_err(|_| Reject::Record)?;
    }
    Ok(bytes)
}
fn require(value: bool, reason: Reject) -> Result<(), Reject> {
    if value { Ok(()) } else { Err(reason) }
}
fn strings(value: Value<'_>, expected: &[&str]) -> Result<bool, Reject> {
    let mut items = value.array()?;
    for text in expected {
        if !items.next().ok_or(Reject::Record)??.equals(text)? {
            return Ok(false);
        }
    }
    Ok(items.next().is_none())
}
fn path_equals(value: Value<'_>, path: &OsStr) -> Result<bool, Reject> {
    let mut expected = path.encode_wide();
    for scalar in value.chars()? {
        let mut buffer = [0; 2];
        for unit in scalar?.encode_utf16(&mut buffer) {
            if expected.next() != Some(*unit) {
                return Ok(false);
            }
        }
    }
    Ok(expected.next().is_none())
}
fn verify_record(record: &[u8], recipe: &[u8], inputs: &Inputs<'_>) -> Result<(), Reject> {
    require(
        <[u8; 32]>::from(Sha256::digest(recipe)) == hex(RECIPE_SHA256)?,
        Reject::Recipe,
    )?;
    let recipe = Value::document(recipe)?;
    let record = Value::document(record)?;
    require(record.get("schemaVersion")?.number()? == 1, Reject::Record)?;
    require(
        record.get("recipeSha256")?.equals(RECIPE_SHA256)?,
        Reject::Recipe,
    )?;
    require(
        record.get("sources")?.0 == recipe.get("sources")?.0,
        Reject::Source,
    )?;
    require(
        record.get("upstreamRoot")?.0 == recipe.get("upstreamRoot")?.0,
        Reject::Source,
    )?;
    require(
        record
            .get("actualGitHeadBaseline")?
            .equals("b72a83bf337c959b3e40c6ef5a255413fc09c0f9")?,
        Reject::Source,
    )?;
    let compiler = record.get("compiler")?;
    require(
        compiler.get("fullCommit")?.equals(model::COMPILER)?
            && compiler.get("release")?.equals("1.99.0")?
            && compiler.get("llvm")?.equals("23.1.1")?,
        Reject::Compiler,
    )?;
    require(
        compiler.get("host")?.equals(model::TARGET)?
            && record.get("target")?.equals(model::TARGET)?,
        Reject::Target,
    )?;
    let flags = record.get("inheritedCompilerFlags")?;
    require(
        flags.get("RUSTFLAGS")?.is("null") && flags.get("CARGO_ENCODED_RUSTFLAGS")?.is("null"),
        Reject::Cfg,
    )?;
    for name in ["actualTargetCfg", "toolchainDefaultTargetCfg"] {
        let cfg = record.get(name)?;
        require(
            cfg.get("panic")?.equals("unwind")?
                && cfg.get("pointerWidth")?.number()? == 64
                && cfg.get("endian")?.equals("little")?
                && cfg.get("raw")?.equals(model::TARGET_CFG)?
                && strings(cfg.get("flags")?, &["debug_assertions", "windows"])?
                && strings(
                    cfg.get("targetFeatures")?,
                    &["cmpxchg16b", "fxsr", "sse", "sse2", "sse3"],
                )?,
            Reject::Cfg,
        )?;
        let values = cfg.get("values")?;
        for (key, value) in [
            ("target_env", "msvc"),
            ("target_os", "windows"),
            ("target_arch", "x86_64"),
            ("target_vendor", "pc"),
            ("target_pointer_width", "64"),
            ("target_endian", "little"),
            ("panic", "unwind"),
            ("target_abi", ""),
        ] {
            require(values.get(key)?.equals(value)?, Reject::Cfg)?;
        }
    }
    let embedded = record.get("embeddedBuildEvidence")?;
    require(
        embedded.0 == recipe.get("embeddedBuildEvidence")?.0,
        Reject::Compiler,
    )?;
    for (key, expected, reason) in [
        (
            "RSHELL_HELPER_BUILD_COMPILER_COMMIT",
            model::COMPILER,
            Reject::Compiler,
        ),
        (
            "RSHELL_HELPER_BUILD_COMPILER_RELEASE",
            "1.99.0",
            Reject::Compiler,
        ),
        ("RSHELL_HELPER_BUILD_TARGET", model::TARGET, Reject::Target),
        (
            "RSHELL_HELPER_BUILD_RESOLVED_FEATURES",
            model::FEATURES,
            Reject::Features,
        ),
        ("RSHELL_HELPER_BUILD_CFG", "normal", Reject::Cfg),
        (
            "RSHELL_HELPER_BUILD_ENVIRONMENT",
            "inherited",
            Reject::Runtime,
        ),
        (
            "RSHELL_HELPER_BUILD_STDIO",
            "synchronous-pipes",
            Reject::Runtime,
        ),
    ] {
        require(embedded.get(key)?.equals(expected)?, reason)?;
    }
    let mut dependencies = record.get("dependencies")?.array()?;
    for (name, version, features) in [
        ("rusqlite", "0.37.0", &["bundled", "modern_sqlite"][..]),
        (
            "libsqlite3-sys",
            "0.35.0",
            &[
                "bundled",
                "bundled_bindings",
                "cc",
                "default",
                "min_sqlite_version_3_14_0",
                "pkg-config",
                "vcpkg",
            ][..],
        ),
        (
            "uuid",
            "1.24.0",
            &["default", "rng", "serde", "std", "v4"][..],
        ),
    ] {
        let dependency = dependencies.next().ok_or(Reject::Features)??;
        require(
            dependency.get("name")?.equals(name)?
                && dependency.get("version")?.equals(version)?
                && strings(dependency.get("enabledFeatures")?, features)?,
            Reject::Features,
        )?;
    }
    require(dependencies.next().is_none(), Reject::Features)?;
    let artifact = record.get("artifact")?;
    require(
        path_equals(artifact.get("path")?, inputs.executable)?
            && artifact.get("sha256")?.equals(ARTIFACT_SHA256)?
            && artifact.get("sizeBytes")?.number()? == 2230272,
        Reject::Artifact,
    )?;
    require(strings(artifact.get("features")?, &[])?, Reject::Features)?;
    let profile = artifact.get("profile")?;
    require(
        profile.get("opt_level")?.equals("0")?
            && profile.get("debuginfo")?.number()? == 2
            && profile.get("debug_assertions")?.is("true")
            && profile.get("overflow_checks")?.is("true")
            && profile.get("test")?.is("false"),
        Reject::Cfg,
    )?;
    for item in recipe.get("sources")?.get("files")?.array()? {
        let item = item?;
        let path = item.get("relativePath")?.plain()?;
        let file = open(inputs.source_root.as_os_str(), Some(path))?;
        let (hash, len) = hash_file(&file)?;
        require(
            hash == hex(item.get("sha256")?.plain()?)? && len == item.get("sizeBytes")?.number()?,
            Reject::Source,
        )?;
    }
    Ok(())
}
