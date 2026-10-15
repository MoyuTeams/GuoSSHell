//! 桌面能力查询：只读配置修订，并在 PATH 中查找系统 ssh；不探测网络、不启动本地程序。

use std::sync::Arc;

use rshell_m0::rshell_storage::SqliteRepository;
use tokio::task::spawn_blocking;

use crate::app::AppContext;
use crate::connect;
use crate::signals::{
    DESKTOP_CONNECT_VERSION, DesktopCapabilitiesQuery, DesktopCapabilitiesResult,
    DesktopCapabilitiesStatus, DesktopCredentialKind, DesktopTargetKind,
    DesktopTransportCapability,
};

const MOBILE_UNAVAILABLE: &str = "移动端不支持桌面会话协议";
const VERSION_UNAVAILABLE: &str = "不支持的桌面会话协议版本";
const STORAGE_UNAVAILABLE: &str = "配置存储不可用";
const SYSTEM_UNAVAILABLE: &str = "未找到系统 ssh（OpenSSH 客户端）";

/// 无请求字段可以提升能力；唯一真实入口取 Rust 的编译目标。
pub async fn handle(
    context: &Arc<AppContext>,
    request: DesktopCapabilitiesQuery,
) -> DesktopCapabilitiesResult {
    let request_id = request.request_id;
    if !connect::desktop_supported() || request.protocol_version != DESKTOP_CONNECT_VERSION {
        return build_for(&context.repository, request, connect::desktop_supported());
    }
    let repository = context.repository.clone();
    spawn_blocking(move || build_for(&repository, request, true))
        .await
        .unwrap_or_else(|_| {
            result(
                request_id,
                DesktopCapabilitiesStatus::StorageUnavailable,
                None,
                false,
            )
        })
}

fn build_for(
    repository: &SqliteRepository,
    request: DesktopCapabilitiesQuery,
    desktop: bool,
) -> DesktopCapabilitiesResult {
    if !desktop {
        return result(
            request.request_id,
            DesktopCapabilitiesStatus::UnsupportedPlatform,
            None,
            false,
        );
    }
    if request.protocol_version != DESKTOP_CONNECT_VERSION {
        return result(
            request.request_id,
            DesktopCapabilitiesStatus::UnsupportedVersion,
            None,
            false,
        );
    }
    match repository.load_configuration() {
        Ok(snapshot) => result(
            request.request_id,
            DesktopCapabilitiesStatus::Ready,
            Some(snapshot.revision),
            connect::system_ssh_found(),
        ),
        Err(_) => result(
            request.request_id,
            DesktopCapabilitiesStatus::StorageUnavailable,
            None,
            false,
        ),
    }
}

/// 为将来传输增加能力时只调整各分支：未通过安全、存储和运行条件的分支保持 false。
/// `system_ssh` 为本次是否找到系统 ssh。
fn result(
    request_id: u32,
    status: DesktopCapabilitiesStatus,
    revision: Option<u64>,
    system_ssh: bool,
) -> DesktopCapabilitiesResult {
    let native_ready = status == DesktopCapabilitiesStatus::Ready;
    let system_ready = native_ready && system_ssh;
    let native_reason = match status {
        DesktopCapabilitiesStatus::Ready => "",
        DesktopCapabilitiesStatus::UnsupportedPlatform => MOBILE_UNAVAILABLE,
        DesktopCapabilitiesStatus::UnsupportedVersion => VERSION_UNAVAILABLE,
        DesktopCapabilitiesStatus::StorageUnavailable => STORAGE_UNAVAILABLE,
    };
    DesktopCapabilitiesResult {
        protocol_version: DESKTOP_CONNECT_VERSION,
        request_id,
        status,
        configuration_revision: revision,
        transports: vec![
            DesktopTransportCapability {
                target: DesktopTargetKind::NativeSsh,
                backend_available: native_ready,
                unavailable_reason: native_reason.into(),
                credentials: if native_ready {
                    vec![
                        DesktopCredentialKind::Password,
                        DesktopCredentialKind::ManagedPublicKey,
                        DesktopCredentialKind::KeyboardInteractive,
                    ]
                } else {
                    vec![]
                },
            },
            // 系统 ssh 自行处理认证（agent、私钥文件、密码均在终端里），应用不提供凭证。
            DesktopTransportCapability {
                target: DesktopTargetKind::SystemOpenSsh,
                backend_available: system_ready,
                unavailable_reason: match (native_ready, system_ready) {
                    (_, true) => "",
                    (true, false) => SYSTEM_UNAVAILABLE,
                    (false, _) => native_reason,
                }
                .into(),
                credentials: vec![],
            },
            // 本地 shell 与 Native 同在桌面目标可用，不需要凭证。
            DesktopTransportCapability {
                target: DesktopTargetKind::LocalShell,
                backend_available: native_ready,
                unavailable_reason: native_reason.into(),
                credentials: vec![],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use rinf::DartSignal;
    use rshell_m0::rshell_storage::SqliteRepository;

    use super::{SYSTEM_UNAVAILABLE, build_for, result};
    use crate::signals::{
        DESKTOP_CONNECT_VERSION, DesktopCapabilitiesQuery, DesktopCapabilitiesStatus,
        DesktopCredentialKind, DesktopTargetKind,
    };

    fn repository() -> SqliteRepository {
        let repository = SqliteRepository::open_in_memory().expect("内存配置存储");
        repository.migrate().expect("初始化配置存储");
        repository
    }

    fn request(request_id: u32) -> DesktopCapabilitiesQuery {
        DesktopCapabilitiesQuery {
            protocol_version: DESKTOP_CONNECT_VERSION,
            request_id,
        }
    }

    #[test]
    fn desktop_query_exposes_only_implemented_backend_and_one_configuration_revision() {
        let repository = repository();
        let first = repository
            .load_configuration()
            .expect("初始配置修订")
            .revision;
        let reply = build_for(&repository, request(31), true);
        assert_eq!(reply.protocol_version, DESKTOP_CONNECT_VERSION);
        assert_eq!(reply.request_id, 31);
        assert_eq!(reply.status, DesktopCapabilitiesStatus::Ready);
        assert_eq!(reply.configuration_revision, Some(first));
        assert_eq!(reply.transports.len(), 3);
        let native = &reply.transports[0];
        assert_eq!(native.target, DesktopTargetKind::NativeSsh);
        assert!(native.backend_available);
        assert!(native.unavailable_reason.is_empty());
        assert_eq!(
            native.credentials,
            [
                DesktopCredentialKind::Password,
                DesktopCredentialKind::ManagedPublicKey,
                DesktopCredentialKind::KeyboardInteractive,
            ]
        );
        let system = &reply.transports[1];
        assert_eq!(system.target, DesktopTargetKind::SystemOpenSsh);
        assert_eq!(system.backend_available, crate::connect::system_ssh_found());
        assert_eq!(
            system.unavailable_reason.is_empty(),
            system.backend_available,
            "不可用时给出原因"
        );
        assert!(system.credentials.is_empty(), "认证由系统 ssh 处理");
        let local = &reply.transports[2];
        assert_eq!(local.target, DesktopTargetKind::LocalShell);
        assert!(local.backend_available);
        assert!(local.unavailable_reason.is_empty());
        assert!(local.credentials.is_empty(), "本地 shell 不需要凭证");
        let mut settings = repository
            .load_configuration()
            .expect("原应用配置")
            .settings;
        settings.key_bindings.clear();
        repository.save_settings(settings).expect("更新配置修订");
        let next = repository
            .load_configuration()
            .expect("新配置修订")
            .revision;
        assert_eq!(next, first + 1);
        assert_eq!(
            build_for(&repository, request(32), true).configuration_revision,
            Some(next)
        );
    }

    #[test]
    fn forged_mobile_request_and_wrong_version_never_read_storage_or_enable_native() {
        let repository = repository();
        repository.shutdown().expect("关闭配置存储");
        let mobile = build_for(&repository, request(40), false);
        assert_eq!(
            mobile.status,
            DesktopCapabilitiesStatus::UnsupportedPlatform
        );
        assert_eq!(mobile.configuration_revision, None);
        assert!(
            mobile
                .transports
                .iter()
                .all(|capability| !capability.backend_available)
        );
        let wrong = build_for(
            &repository,
            DesktopCapabilitiesQuery {
                protocol_version: DESKTOP_CONNECT_VERSION + 1,
                request_id: 41,
            },
            true,
        );
        assert_eq!(wrong.status, DesktopCapabilitiesStatus::UnsupportedVersion);
        assert_eq!(wrong.configuration_revision, None);
        assert!(
            wrong
                .transports
                .iter()
                .all(|capability| !capability.backend_available)
        );
        let storage = build_for(&repository, request(42), true);
        assert_eq!(
            storage.status,
            DesktopCapabilitiesStatus::StorageUnavailable
        );
        assert_eq!(storage.configuration_revision, None);
        assert!(
            storage
                .transports
                .iter()
                .all(|capability| !capability.backend_available)
        );
    }

    #[tokio::test]
    async fn rinf_query_keeps_request_id_and_serializes_only_capabilities() {
        let receiver = DesktopCapabilitiesQuery::get_dart_signal_receiver();
        let query = request(77);
        let wire = rinf::serialize(&query).expect("桌面能力请求编码");
        crate::signals::inject_desktop_capabilities_query(&wire);
        let received = receiver.recv().await.expect("能力信号").message;
        let repository = repository();
        let reply = build_for(&repository, received, true);
        assert_eq!(reply.request_id, 77);
        let wire = rinf::serialize(&reply).expect("桌面能力结果编码");
        assert!(!wire.is_empty());
    }

    #[test]
    fn unavailable_transports_keep_fixed_reasons_for_future_implementation() {
        let ready = result(1, DesktopCapabilitiesStatus::Ready, Some(1), true);
        assert!(ready.transports[1].backend_available);
        assert!(ready.transports[1].unavailable_reason.is_empty());
        let missing = result(1, DesktopCapabilitiesStatus::Ready, Some(1), false);
        assert!(!missing.transports[1].backend_available);
        assert_eq!(missing.transports[1].unavailable_reason, SYSTEM_UNAVAILABLE);
        // 存储不可用时找到 ssh 也不开放，原因与 Native 相同。
        let storage = result(2, DesktopCapabilitiesStatus::StorageUnavailable, None, true);
        assert!(!storage.transports[1].backend_available);
        assert_eq!(
            storage.transports[1].unavailable_reason,
            storage.transports[0].unavailable_reason
        );
        // 本地 shell 与 Native 同条件可用，不可用时沿用同一原因。
        assert_eq!(
            storage.transports[2].unavailable_reason,
            storage.transports[0].unavailable_reason
        );
        assert!(!storage.transports[2].backend_available);
    }
}
