//! 先借用、计数，再在单个已准入 backing 中编码；不执行本机 shell。

use std::path::{Component, Path};

use super::{ScopeReject, SourceBinding};

pub const ARG_CAPACITY: usize = 12;
pub const ENV_CAPACITY: usize = 18;
pub const POST_AUTH_COMMAND: &str = "guo-system-post-auth-v1";

const POLICY: &str = concat!(
    "CanonicalizeHostname no\n",
    "StrictHostKeyChecking yes\n",
    "UserKnownHostsFile none\n",
    "GlobalKnownHostsFile none\n",
    "UpdateHostKeys no\n",
    "VerifyHostKeyDNS no\n",
    "CheckHostIP no\n",
    "NoHostAuthenticationForLocalhost no\n",
    "IdentityAgent none\n",
    "IdentitiesOnly yes\n",
    "AddKeysToAgent no\n",
    "ForwardAgent no\n",
    "KbdInteractiveAuthentication no\n",
    "HostbasedAuthentication no\n",
    "GSSAPIAuthentication no\n",
    "GSSAPIDelegateCredentials no\n",
    "EnableSSHKeysign no\n",
    "PKCS11Provider none\n",
    "SecurityKeyProvider ${GUO_SYSTEM_PROVIDER}\n",
    "CertificateFile ${GUO_SYSTEM_CERTIFICATE}\n",
    "RevokedHostKeys none\n",
    "ControlMaster no\n",
    "ControlPath none\n",
    "ControlPersist no\n",
    "ForkAfterAuthentication no\n",
    "ProxyCommand none\n",
    "ProxyJump none\n",
    "ProxyUseFdpass no\n",
    "ClearAllForwardings yes\n",
    "ForwardX11 no\n",
    "ForwardX11Trusted no\n",
    "PermitLocalCommand yes\n",
    "LocalCommand guo-system-post-auth-v1\n",
    "EscapeChar none\n",
    "RequestTTY force\n",
    "Tunnel no\n",
    "ConnectionAttempts 1\n",
    "ConnectTimeout 15\n",
    "ServerAliveInterval 15\n",
    "ServerAliveCountMax 3\n",
    "LogLevel ERROR\n",
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopPlatform {
    Windows,
    MacOs,
    Linux,
}

impl DesktopPlatform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// 文件路径不是授权；父保留实际选文件／沙箱生命周期并证明子进程可重开。
///
/// # Safety
/// 实现者只能为本次直接显选的精确文件集合返回许可，不得以目录、已保存路径、
/// 父进程可读或产品设计授权代替子进程实际文件许可。借用期间许可必须存活。
pub unsafe trait SelectedFileAccess {
    fn permits_ssh_reopen(&self, exact_path: &str, binding: SourceBinding) -> bool;
}

#[derive(Clone, Copy)]
pub struct ExternalIdentity<'a> {
    identity: &'a str,
    public_companion: &'a str,
    access: &'a dyn SelectedFileAccess,
}

impl<'a> ExternalIdentity<'a> {
    pub fn selected(
        identity: &'a str,
        public_companion: &'a str,
        access: &'a dyn SelectedFileAccess,
    ) -> Result<Self, ScopeReject> {
        absolute_path(identity)?;
        absolute_path(public_companion)?;
        if public_companion.strip_suffix(".pub") != Some(identity) {
            return Err(ScopeReject::ExactFileSelectionRequired);
        }
        Ok(Self {
            identity,
            public_companion,
            access,
        })
    }

    pub fn exact_files(self) -> [&'a str; 2] {
        [self.identity, self.public_companion]
    }

    fn check(self, binding: SourceBinding) -> Result<(), ScopeReject> {
        for path in self.exact_files() {
            if !self.access.permits_ssh_reopen(path, binding) {
                return Err(ScopeReject::FilePermissionNotProven);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum SystemAuth<'a> {
    Password,
    ExternalFile(ExternalIdentity<'a>),
}

#[derive(Clone, Copy)]
pub struct PrivatePaths<'a> {
    pub cwd: &'a str,
    pub home: &'a str,
    pub config: &'a str,
    pub empty_certificate: &'a str,
    pub empty_provider: &'a str,
    pub helper: &'a str,
    pub endpoint: &'a str,
}

/// 母 owner 的私有资源借用；leaf 不凭绝对路径宣称目录已隔离/占位文件已存在。
///
/// # Safety
/// 必须持有本次私有目录、config、零长度 certificate/provider 的真实所有权及清理登记，
/// cwd 中没有用户来源文件，资源不能为指向用户文件的重定向。借用期间不允许外部改写；
/// config 只能接收本模块的封闭模板，持久 System trust 属于独立 owner，不能在此删除。
pub unsafe trait PrivateAttemptFiles {
    fn owns_private_attempt(&self, paths: PrivatePaths<'_>, binding: SourceBinding) -> bool;
}

/// SystemRoot 来自宿主 OS API，不从用户环境变量推断；仅借用其只读结果。
///
/// # Safety
/// 实现者须验证 OS 来源和不可被用户替换的系统路径，不得返回任意调用者环境值。
pub unsafe trait TrustedOsPaths {
    fn system_root(&self) -> Option<&str>;
}

#[derive(Clone, Copy)]
pub struct BorrowedSystem<'a> {
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub remote_command: Option<&'a str>,
    pub term: &'a str,
    pub auth: SystemAuth<'a>,
    pub paths: PrivatePaths<'a>,
    pub platform: DesktopPlatform,
    pub os: &'a dyn TrustedOsPaths,
    pub private_files: &'a dyn PrivateAttemptFiles,
    pub binding: SourceBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchPurpose {
    Version,
    EffectiveConfig,
    Session,
}

#[derive(Clone, Copy)]
pub struct EnvironmentEntry<'a> {
    pub key: &'static str,
    pub value: &'a str,
}

/// 无任意 env 添加接口，无继承／合并；秘密不属于此编码面。
pub struct EnvClearExplicitBuilder<'a> {
    entries: [Option<EnvironmentEntry<'a>>; ENV_CAPACITY],
    len: usize,
}

impl<'a> EnvClearExplicitBuilder<'a> {
    pub fn for_system(
        input: BorrowedSystem<'a>,
        purpose: LaunchPurpose,
    ) -> Result<Self, ScopeReject> {
        let mut result = Self {
            entries: [None; ENV_CAPACITY],
            len: 0,
        };
        result.add("HOME", input.paths.home)?;
        result.add("LANG", "C")?;
        result.add("LC_ALL", "C")?;
        result.add("TERM", input.term)?;
        result.add("GUO_SYSTEM_CERTIFICATE", input.paths.empty_certificate)?;
        result.add("GUO_SYSTEM_PROVIDER", input.paths.empty_provider)?;
        if let SystemAuth::ExternalFile(file) = input.auth {
            result.add("GUO_SYSTEM_IDENTITY", file.identity)?;
        }
        if input.platform == DesktopPlatform::Windows {
            let system_root = input
                .os
                .system_root()
                .ok_or(ScopeReject::AbsolutePathRequired)?;
            absolute_path(system_root)?;
            result.add("SystemRoot", system_root)?;
            result.add("USERPROFILE", input.paths.home)?;
            result.add("APPDATA", input.paths.home)?;
            result.add("LOCALAPPDATA", input.paths.home)?;
            // Windows OpenSSH 的启动器要求该变量；指向私有目录而非系统信任/配置目录。
            result.add("ProgramData", input.paths.home)?;
            result.add("TEMP", input.paths.cwd)?;
            result.add("TMP", input.paths.cwd)?;
        } else {
            result.add("TMPDIR", input.paths.cwd)?;
        }
        // -V/-G 不执行 helper；仅 session 重建固定回调所需白名单。
        if purpose == LaunchPurpose::Session {
            result.add("SSH_ASKPASS", input.paths.helper)?;
            result.add("SSH_ASKPASS_REQUIRE", "force")?;
            result.add(
                if input.platform == DesktopPlatform::Windows {
                    "COMSPEC"
                } else {
                    "SHELL"
                },
                input.paths.helper,
            )?;
            result.add("GUO_SYSTEM_ENDPOINT", input.paths.endpoint)?;
        }
        Ok(result)
    }

    fn add(&mut self, key: &'static str, value: &'a str) -> Result<(), ScopeReject> {
        text(value)?;
        let slot = self
            .entries
            .get_mut(self.len)
            .ok_or(ScopeReject::Encoding)?;
        *slot = Some(EnvironmentEntry { key, value });
        self.len += 1;
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = EnvironmentEntry<'a>> + '_ {
        self.entries[..self.len].iter().flatten().copied()
    }
}

pub(crate) fn text(value: &str) -> Result<(), ScopeReject> {
    if value.is_empty() {
        return Err(ScopeReject::EmptyField);
    }
    if value.chars().any(char::is_control) {
        return Err(ScopeReject::ControlCharacter);
    }
    Ok(())
}

pub(crate) fn absolute_path(value: &str) -> Result<(), ScopeReject> {
    text(value)?;
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(ScopeReject::AbsolutePathRequired);
    }
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(ScopeReject::InvalidPrivateLayout);
    }
    #[cfg(windows)]
    {
        // UNC、设备名及 ADS 不属于本单元的本地普通文件授权模型。
        let bytes = value.as_bytes();
        if bytes.len() < 3
            || bytes[1] != b':'
            || !bytes[0].is_ascii_alphabetic()
            || !matches!(bytes[2], b'/' | b'\\')
            || value[2..].contains(':')
            || value
                .chars()
                .any(|c| matches!(c, '"' | '<' | '>' | '|' | '?' | '*'))
        {
            return Err(ScopeReject::AbsolutePathRequired);
        }
        for component in path.components() {
            if let Component::Normal(name) = component {
                let name = name.to_str().ok_or(ScopeReject::Encoding)?;
                if name.ends_with(['.', ' ']) {
                    return Err(ScopeReject::AbsolutePathRequired);
                }
                let base = name.split('.').next().ok_or(ScopeReject::Encoding)?;
                if ["CON", "PRN", "AUX", "NUL"]
                    .iter()
                    .any(|reserved| base.eq_ignore_ascii_case(reserved))
                    || (base.len() == 4
                        && ["COM", "LPT"].iter().any(|prefix| {
                            base.as_bytes()[..3].eq_ignore_ascii_case(prefix.as_bytes())
                        })
                        && matches!(base.as_bytes()[3], b'1'..=b'9'))
                {
                    return Err(ScopeReject::AbsolutePathRequired);
                }
            }
        }
    }
    Ok(())
}

impl BorrowedSystem<'_> {
    pub fn validate(self) -> Result<(), ScopeReject> {
        text(self.host)?;
        if self.host.starts_with('-') {
            return Err(ScopeReject::LeadingOption);
        }
        if !self
            .host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | ':' | '%' | '_'))
            || self.host.starts_with('.')
            || self.host.contains("..")
        {
            return Err(ScopeReject::InvalidHost);
        }
        if self.host.contains(':') {
            let address = self
                .host
                .split_once('%')
                .map_or(self.host, |(address, _)| address);
            if address.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(ScopeReject::InvalidHost);
            }
            if let Some((_, zone)) = self.host.split_once('%')
                && (zone.is_empty()
                    || !zone
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
            {
                return Err(ScopeReject::InvalidHost);
            }
        } else if self.host.contains('%') {
            return Err(ScopeReject::InvalidHost);
        }
        if self.port == 0 {
            return Err(ScopeReject::InvalidPort);
        }
        text(self.username)?;
        if self.username.starts_with('-') {
            return Err(ScopeReject::LeadingOption);
        }
        if let Some(command) = self.remote_command {
            text(command)?;
        }
        text(self.term)?;
        if !self
            .term
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'+' | b'.'))
        {
            return Err(ScopeReject::InvalidTerm);
        }
        for path in [
            self.paths.cwd,
            self.paths.home,
            self.paths.config,
            self.paths.empty_certificate,
            self.paths.empty_provider,
            self.paths.helper,
        ] {
            absolute_path(path)?;
        }
        let cwd = Path::new(self.paths.cwd);
        for path in [
            self.paths.home,
            self.paths.config,
            self.paths.empty_certificate,
            self.paths.empty_provider,
        ] {
            if Path::new(path).parent() != Some(cwd) {
                return Err(ScopeReject::InvalidPrivateLayout);
            }
        }
        if self.paths.config == self.paths.empty_certificate
            || self.paths.config == self.paths.empty_provider
            || self.paths.empty_provider == self.paths.empty_certificate
        {
            return Err(ScopeReject::InvalidPrivateLayout);
        }
        text(self.paths.endpoint)?;
        if !self
            .private_files
            .owns_private_attempt(self.paths, self.binding)
        {
            return Err(ScopeReject::PrivateAttemptUnavailable);
        }
        if let SystemAuth::ExternalFile(file) = self.auth {
            file.check(self.binding)?;
        }
        Ok(())
    }
}

pub(crate) trait Sink {
    fn put(&mut self, value: &str) -> Result<(), ScopeReject>;
}

pub(crate) struct Counter(pub usize);
impl Sink for Counter {
    fn put(&mut self, value: &str) -> Result<(), ScopeReject> {
        self.0 = self
            .0
            .checked_add(value.len())
            .ok_or(ScopeReject::Encoding)?;
        Ok(())
    }
}

pub(crate) struct Writer<'a> {
    pub bytes: &'a mut [u8],
    pub used: usize,
}
impl Sink for Writer<'_> {
    fn put(&mut self, value: &str) -> Result<(), ScopeReject> {
        let end = self
            .used
            .checked_add(value.len())
            .ok_or(ScopeReject::Encoding)?;
        self.bytes
            .get_mut(self.used..end)
            .ok_or(ScopeReject::Encoding)?
            .copy_from_slice(value.as_bytes());
        self.used = end;
        Ok(())
    }
}

/// command 指令保留整段文本，只有 callback 的 argv_split 引号层。
/// executable av0 不做 percent/dollar 展开；路径只编码引号/反斜杠，literal %/$ 原样保留。
/// 后续参数分别使用 OpenSSH 的 %I/%H/%t/%K token，不能与 av0 共用展开编码。
fn helper_command(sink: &mut impl Sink, helper: &str) -> Result<(), ScopeReject> {
    sink.put("KnownHostsCommand \"")?;
    for c in helper.chars() {
        match c {
            '\\' => sink.put("\\\\")?,
            '"' => sink.put("\\\"")?,
            _ => {
                let mut buffer = [0; 4];
                sink.put(c.encode_utf8(&mut buffer))?;
            }
        }
    }
    sink.put("\" host-key \"%I\" \"%H\" \"%t\" \"%K\"\n")
}

pub(crate) fn config(sink: &mut impl Sink, input: BorrowedSystem<'_>) -> Result<(), ScopeReject> {
    sink.put(POLICY)?;
    helper_command(sink, input.paths.helper)?;
    match input.auth {
        SystemAuth::Password => sink.put("PreferredAuthentications password\nPasswordAuthentication yes\nPubkeyAuthentication no\nIdentityFile none\n"),
        // 固定环境名经实际版本的单次展开；路径中的 %、${}、~、引号不被重新解释。
        // 真正文件加载前仍须获得该版本的非递归展开及访问集合证据，-G 不证明它。
        SystemAuth::ExternalFile(_) => sink.put("PreferredAuthentications publickey\nPasswordAuthentication no\nPubkeyAuthentication yes\nIdentityFile ${GUO_SYSTEM_IDENTITY}\n"),
    }
}

pub(crate) fn with_args<R>(
    input: BorrowedSystem<'_>,
    purpose: LaunchPurpose,
    use_args: impl FnOnce(&[&str]) -> R,
) -> R {
    let mut port = [0; 5];
    let mut number = input.port;
    let mut first = 5;
    loop {
        first -= 1;
        port[first] = b'0' + (number % 10) as u8;
        number /= 10;
        if number == 0 {
            break;
        }
    }
    // SAFETY: 只写入 ASCII 数字。
    let port = unsafe { std::str::from_utf8_unchecked(&port[first..]) };
    let mut args = [""; ARG_CAPACITY];
    let mut len = 0;
    if purpose == LaunchPurpose::Version {
        return use_args(&["-V"]);
    }
    for arg in ["-F", input.paths.config, "-l", input.username, "-p", port] {
        args[len] = arg;
        len += 1;
    }
    args[len] = if purpose == LaunchPurpose::EffectiveConfig {
        "-G"
    } else {
        "-tt"
    };
    len += 1;
    args[len] = "--";
    len += 1;
    args[len] = input.host;
    len += 1;
    if purpose == LaunchPurpose::Session
        && let Some(command) = input.remote_command
    {
        args[len] = command;
        len += 1;
    }
    use_args(&args[..len])
}

/// Windows CRT 参数编码；不是 cmd.exe／PowerShell 命令语言。
pub fn windows_crt_argument(
    value: &str,
    mut emit: impl FnMut(u16) -> Result<(), ScopeReject>,
) -> Result<(), ScopeReject> {
    emit(b'"' as u16)?;
    let mut slashes = 0usize;
    for unit in value.encode_utf16() {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        let count = if unit == b'"' as u16 {
            slashes
                .checked_mul(2)
                .and_then(|n| n.checked_add(1))
                .ok_or(ScopeReject::Encoding)?
        } else {
            slashes
        };
        for _ in 0..count {
            emit(b'\\' as u16)?;
        }
        emit(unit)?;
        slashes = 0;
    }
    for _ in 0..slashes.checked_mul(2).ok_or(ScopeReject::Encoding)? {
        emit(b'\\' as u16)?;
    }
    emit(b'"' as u16)
}
