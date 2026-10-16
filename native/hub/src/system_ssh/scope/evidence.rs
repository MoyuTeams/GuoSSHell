//! 证据分层；配置解析既不认证，也不证明文件访问或回调身份。

use crate::selected_p1::resource::ledger::SourceToken;

use super::{ScopeReject, SystemAuth};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceBinding {
    pub source: SourceToken,
    pub attempt: u64,
    pub target_identity: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceLevel {
    StaticDeclared,
    Queried,
    ActualProcessVerified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HookAttributes {
    pub known_hosts_command: EvidenceLevel,
    pub force_askpass_with_pty: EvidenceLevel,
    pub post_auth_shell_chain: EvidenceLevel,
    pub nonrecursive_path_expansion: EvidenceLevel,
    pub exact_identity_file_access: EvidenceLevel,
    pub no_user_or_global_config_access: EvidenceLevel,
}

impl Default for HookAttributes {
    fn default() -> Self {
        Self {
            known_hosts_command: EvidenceLevel::StaticDeclared,
            force_askpass_with_pty: EvidenceLevel::StaticDeclared,
            post_auth_shell_chain: EvidenceLevel::StaticDeclared,
            nonrecursive_path_expansion: EvidenceLevel::StaticDeclared,
            exact_identity_file_access: EvidenceLevel::StaticDeclared,
            no_user_or_global_config_access: EvidenceLevel::StaticDeclared,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ActualVersion<'a> {
    line: &'a str,
}

impl<'a> ActualVersion<'a> {
    /// 内容识别，不证明调用来源；版本与锁定映像的关联由父 QueryProcessProof 保持。
    pub fn parse(output: &'a [u8]) -> Result<Self, ScopeReject> {
        if output.len() > 1024 {
            return Err(ScopeReject::QueryTooLarge);
        }
        let line = std::str::from_utf8(output)
            .map_err(|_| ScopeReject::Encoding)?
            .trim_end_matches(['\r', '\n']);
        if line.chars().any(char::is_control)
            || !(line.starts_with("OpenSSH_") || line.starts_with("OpenSSH_for_Windows_"))
        {
            return Err(ScopeReject::VersionNotOpenSsh);
        }
        Ok(Self { line })
    }
    pub fn line(self) -> &'a str {
        self.line
    }
}

pub struct EffectiveQuery<'a> {
    text: &'a str,
}

impl<'a> EffectiveQuery<'a> {
    pub fn parse(output: &'a [u8]) -> Result<Self, ScopeReject> {
        if output.len() > 32 * 1024 {
            return Err(ScopeReject::QueryTooLarge);
        }
        let text = std::str::from_utf8(output).map_err(|_| ScopeReject::Encoding)?;
        if text.contains('\0') {
            return Err(ScopeReject::Encoding);
        }
        Ok(Self { text })
    }

    pub fn values(&self, name: &'a str) -> impl Iterator<Item = &'a str> + '_ {
        self.text.lines().filter_map(move |line| {
            let (key, value) = line.split_once(' ')?;
            (key == name).then_some(value.trim_end_matches('\r'))
        })
    }

    pub fn exactly(&self, name: &'a str, value: &str) -> Result<(), ScopeReject> {
        let mut found = self.values(name);
        if found.next() != Some(value) || found.next().is_some() {
            return Err(ScopeReject::QueryMismatch);
        }
        Ok(())
    }

    pub fn verify_closed_policy(
        &self,
        auth: SystemAuth<'_>,
    ) -> Result<HookAttributes, ScopeReject> {
        for (key, value) in [
            ("stricthostkeychecking", "true"),
            ("userknownhostsfile", "none"),
            ("globalknownhostsfile", "none"),
            ("updatehostkeys", "false"),
            ("verifyhostkeydns", "false"),
            ("checkhostip", "no"),
            ("canonicalizehostname", "false"),
            ("identityagent", "none"),
            ("identitiesonly", "yes"),
            ("addkeystoagent", "false"),
            ("forwardagent", "no"),
            ("kbdinteractiveauthentication", "no"),
            ("hostbasedauthentication", "no"),
            ("controlmaster", "false"),
            ("controlpersist", "no"),
            ("forkafterauthentication", "no"),
            ("clearallforwardings", "yes"),
            ("forwardx11", "no"),
            ("forwardx11trusted", "no"),
            ("permitlocalcommand", "yes"),
            ("localcommand", super::POST_AUTH_COMMAND),
            ("escapechar", "none"),
            ("requesttty", "force"),
            ("certificatefile", "${GUO_SYSTEM_CERTIFICATE}"),
        ] {
            self.exactly(key, value)?;
        }
        for key in [
            "controlpath",
            "proxycommand",
            "proxyjump",
            "revokedhostkeys",
            "pkcs11provider",
        ] {
            let mut values = self.values(key);
            if let Some(value) = values.next()
                && (value != "none" || values.next().is_some())
            {
                return Err(ScopeReject::QueryMismatch);
            }
        }
        for key in [
            "gssapiauthentication",
            "gssapidelegatecredentials",
            "enablesshkeysign",
        ] {
            let mut values = self.values(key);
            if let Some(value) = values.next()
                && (value != "no" || values.next().is_some())
            {
                return Err(ScopeReject::QueryMismatch);
            }
        }
        for key in [
            "localforward",
            "remoteforward",
            "dynamicforward",
            "sendenv",
            "setenv",
        ] {
            if self.values(key).next().is_some() {
                return Err(ScopeReject::QueryMismatch);
            }
        }
        let (preferred, password, publickey, identity) = match auth {
            SystemAuth::Password => ("password", "yes", "false", "none"),
            SystemAuth::ExternalFile(_) => ("publickey", "no", "true", "${GUO_SYSTEM_IDENTITY}"),
        };
        for (key, value) in [
            ("preferredauthentications", preferred),
            ("passwordauthentication", password),
            ("pubkeyauthentication", publickey),
            ("identityfile", identity),
        ] {
            self.exactly(key, value)?;
        }
        if self.values("knownhostscommand").next().is_none() {
            return Err(ScopeReject::QueryMismatch);
        }
        // dump_config 不打印某些 none/unsupported 选项；它们不能据此升级成运行证据。
        Ok(HookAttributes {
            known_hosts_command: EvidenceLevel::Queried,
            ..HookAttributes::default()
        })
    }
}

/// 父持有的真实 query/认证子链证据；只读、不产生探针或访问用户文件。
///
/// # Safety
/// 必须绑定当前锁定映像、实际版本、source/attempt、PID/不可复用句柄及真实回调。
/// -V/-G、手册字段、fixture label、PID 单值和输出文字都不能升级成实际回调证据。
pub unsafe trait ParentProcessProof {
    fn matches(&self, image: &super::ExeIdentity<'_>, binding: SourceBinding) -> bool;
    fn hooks(&self) -> HookAttributes;
}

pub fn require_session_hooks(
    image: &super::ExeIdentity<'_>,
    binding: SourceBinding,
    auth: SystemAuth<'_>,
    proof: &dyn ParentProcessProof,
) -> Result<(), ScopeReject> {
    image.revalidate()?;
    if !proof.matches(image, binding) {
        return Err(ScopeReject::ImageChanged);
    }
    let hooks = proof.hooks();
    let actual = EvidenceLevel::ActualProcessVerified;
    if hooks.known_hosts_command != actual
        || hooks.post_auth_shell_chain != actual
        || hooks.force_askpass_with_pty != actual
        || hooks.nonrecursive_path_expansion != actual
        || hooks.no_user_or_global_config_access != actual
        || (matches!(auth, SystemAuth::ExternalFile(_))
            && hooks.exact_identity_file_access != actual)
    {
        return Err(ScopeReject::HookNotVerified);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemProgress {
    Starting,
    Handshake,
    AwaitingHostDecision,
    AwaitingCredential,
    AuthenticatedRequestingTerminal,
    Ended,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthObservation {
    NoneMethodProbe,
    CredentialRequested,
    PostAuthCallback,
}

impl SystemProgress {
    pub fn observe(self, observation: AuthObservation) -> Self {
        if self == Self::Ended {
            return Self::Ended;
        }
        match observation {
            AuthObservation::NoneMethodProbe => self,
            AuthObservation::CredentialRequested => Self::AwaitingCredential,
            // 此枚举只表达状态，不验证回调；父须先完成 require_session_hooks 与事件身份验证。
            AuthObservation::PostAuthCallback => Self::AuthenticatedRequestingTerminal,
        }
    }
}
