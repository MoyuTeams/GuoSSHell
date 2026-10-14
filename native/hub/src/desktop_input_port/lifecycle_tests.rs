#![allow(clippy::expect_used)]
use super::*;
use crate::signals::desktop_native_input::{NativeCommand, VERSION};

fn query(port: &InputPort, version: u16) -> DesktopNativeLifecycleQueryResult {
    query_request(DesktopNativeLifecycleQuery {
        version,
        source: port.source.source.clone(),
        identity: port.source.identity.clone(),
        request_id: 7,
    })
}

fn query_request(request: DesktopNativeLifecycleQuery) -> DesktopNativeLifecycleQueryResult {
    let _isolation = tests::isolate_source_registration();
    query_lifecycle(request)
}

#[test]
fn lifecycle_query_with_registration_contention_is_busy_and_read_only() {
    let root = tests::registration(8403);
    let port = &root.0;
    port.begin_attempt();
    port.connected();
    let binding = port.binding();
    let guard = tests::hold_source_registration();
    let response = query_lifecycle(DesktopNativeLifecycleQuery {
        version: VERSION,
        source: port.source.source.clone(),
        identity: port.source.identity.clone(),
        request_id: 7,
    });
    assert_eq!(response.failure, NativeLifecycleQueryFailure::Busy);
    assert!(response.lifecycle.is_none());
    assert!(matches!(
        lookup(&port.source.source, &port.source.identity),
        Err(Reject::Busy)
    ));
    drop(guard);
    assert_eq!(port.binding(), binding);
    assert_eq!(
        query(port, VERSION).lifecycle.expect("认证投影").state,
        NativeLifecycleState::Authenticated
    );
    assert!(port.projection().is_none());
    assert!(matches!(port.attach(binding), Err(Reject::NotAttached)));
    assert!(port.mode.legacy());
}

#[test]
fn lifecycle_query_before_first_frame_is_read_only_and_cannot_authorize_keys() {
    let root = tests::registration(8401);
    let port = &root.0;
    port.begin_attempt();
    let binding = port.binding();
    let response = query(port, VERSION);
    assert_eq!(response.failure, NativeLifecycleQueryFailure::None);
    assert_eq!(response.request_id, 7);
    let snapshot = response.lifecycle.expect("首帧前投影");
    assert_eq!(snapshot.state, NativeLifecycleState::Connecting);
    assert_eq!(snapshot.attempt, binding.attempt);
    assert_eq!(snapshot.focus_epoch, binding.epoch);
    assert!(port.projection().is_none());
    assert!(matches!(port.attach(binding), Err(Reject::NotAttached)));
    port.connected();
    let authenticated = query(port, VERSION).lifecycle.expect("认证投影");
    assert_eq!(authenticated.state, NativeLifecycleState::Authenticated);
    assert!(matches!(port.attach(binding), Err(Reject::NotAttached)));
    assert_eq!(
        port.command(
            binding,
            1,
            NativeCommand::Event(DesktopEvent::Key {
                key: "a".into(),
                physical: 1,
                logical: 1,
                shift: false,
                control: false,
                alt: false,
                super_key: false,
                phase: DesktopKeyPhase::Down,
                left_alt: false,
                right_alt: false,
                alt_source: crate::signals::desktop_session::DesktopAltSource::Unknown,
                default_action: None,
            })
        ),
        Err(Reject::InvalidEvent)
    );
    assert_eq!(port.binding(), binding);
    assert_eq!(
        query(port, VERSION).lifecycle.expect("重复查询"),
        authenticated
    );
    assert_eq!(
        query(port, VERSION + 1).failure,
        NativeLifecycleQueryFailure::UnsupportedVersion
    );
    let mismatch = query_request(DesktopNativeLifecycleQuery {
        version: VERSION,
        source: port.source.source.clone(),
        identity: "其他来源".into(),
        request_id: 7,
    });
    assert_eq!(mismatch.failure, NativeLifecycleQueryFailure::StaleSource);
    assert!(mismatch.lifecycle.is_none());
    assert!(port.mode.legacy(), "只读生命周期不选择原生输入模式");
}

#[test]
fn verified_reattach_restores_authenticated_lifecycle_without_reviving_old_surface() {
    let root = tests::registration(8404);
    let port = &root.0;
    port.begin_attempt();
    port.connected();
    let authenticated = query(port, VERSION).lifecycle.expect("已认证投影");
    let mut events = port.observe_lifecycle();
    // 仅受控测试宿主满足 unsafe 接缝；生命周期查询本身不登记宿主。
    let surface = unsafe {
        port.attach_verified(authenticated.attempt)
            .expect("首次宿主登记")
    };
    let attached = events.try_recv().expect("首次登记通知");
    assert_eq!(attached.state, NativeLifecycleState::Authenticated);
    assert!(attached.focus_epoch > authenticated.focus_epoch);
    let callback = surface.clone();
    surface.revoke().expect("宿主撤权");
    let revoked = events.try_recv().expect("撤权通知");
    assert_eq!(revoked.state, NativeLifecycleState::InputRevoked);
    assert!(port.state.lock().expect("state").authenticated);
    // 原宿主停止回调并释放自身引用；迟到的 callback clone 仍只能指向旧 surface。
    drop(surface);
    assert!(matches!(
        unsafe { port.attach_verified(authenticated.attempt + 1) },
        Err(Reject::StaleAttempt)
    ));
    assert!(matches!(
        port.attach(callback.binding()),
        Err(Reject::StaleFocus)
    ));
    assert_eq!(query(port, VERSION).lifecycle.expect("未重新登记"), revoked);
    assert!(events.try_recv().is_err());

    let current = unsafe {
        port.attach_verified(authenticated.attempt)
            .expect("同 attempt 再登记")
    };
    let restored = events.try_recv().expect("重新登记通知");
    assert_eq!(restored.source, authenticated.source);
    assert_eq!(restored.identity, authenticated.identity);
    assert_eq!(restored.attempt, authenticated.attempt);
    assert!(restored.focus_epoch > revoked.focus_epoch);
    assert!(restored.lifecycle_revision > revoked.lifecycle_revision);
    assert_eq!(restored.state, NativeLifecycleState::Authenticated);
    assert_eq!(restored.focus_epoch, current.binding().epoch);
    assert_eq!(
        query(port, VERSION).lifecycle.expect("重新登记投影"),
        restored
    );
    assert_eq!(
        callback.key(tests::key(1, DesktopKeyPhase::Down)),
        Err(Reject::StaleFocus)
    );
    assert_eq!(callback.revoke(), Err(Reject::StaleFocus));
    assert!(matches!(
        unsafe { port.attach_verified(authenticated.attempt) },
        Err(Reject::Busy)
    ));
    assert_eq!(
        query(port, VERSION).lifecycle.expect("旧宿主不撤新权"),
        restored
    );
    assert!(events.try_recv().is_err());
    current
        .key(tests::key(1, DesktopKeyPhase::Down))
        .expect("新宿主输入");
    current.revoke().expect("新宿主退休");
}

#[test]
fn verified_attach_without_authentication_preserves_lifecycle_and_input_rejection() {
    for (id, lifecycle) in [
        (8405, NativeLifecycleState::Connecting),
        (8406, NativeLifecycleState::AuthenticationFailed),
        (8407, NativeLifecycleState::AttemptFailed),
        (8408, NativeLifecycleState::Disconnected),
    ] {
        let root = tests::registration(id);
        let port = &root.0;
        port.begin_attempt();
        match lifecycle {
            NativeLifecycleState::AuthenticationFailed => port.attempt_failed(true),
            NativeLifecycleState::AttemptFailed => port.attempt_failed(false),
            NativeLifecycleState::Disconnected => {
                port.connected();
                port.disconnected();
            }
            _ => {}
        }
        let before = query(port, VERSION).lifecycle.expect("未认证投影");
        let mut events = port.observe_lifecycle();
        let surface = unsafe { port.attach_verified(before.attempt).expect("受控宿主登记") };
        let attached = events.try_recv().expect("宿主登记通知");
        assert_eq!(attached.state, lifecycle);
        assert_eq!(attached.attempt, before.attempt);
        assert!(attached.focus_epoch > before.focus_epoch);
        assert_eq!(
            surface.key(tests::key(1, DesktopKeyPhase::Down)),
            Err(Reject::NotAuthenticated)
        );
        assert!(!port.state.lock().expect("state").authenticated);
    }
}

#[test]
fn lifecycle_revisions_track_revocation_and_old_source_never_revives() {
    let root = tests::registration(8402);
    let port = root.0.clone();
    let mut events = port.observe_lifecycle();
    port.begin_attempt();
    port.connected();
    // 仅受控测试宿主能登记；查询并未授予这项权限。
    let surface = unsafe {
        port.attach_verified(port.binding().attempt)
            .expect("测试宿主")
    };
    let previous = surface.binding();
    surface.revoke().expect("原宿主撤权");
    assert_eq!(
        query(&port, VERSION).lifecycle.expect("撤权投影").state,
        NativeLifecycleState::InputRevoked
    );
    assert_eq!(
        surface.key(tests::key(1, DesktopKeyPhase::Down)),
        Err(Reject::StaleFocus)
    );
    port.disconnected();
    port.begin_attempt();
    let current = query(&port, VERSION).lifecycle.expect("重连投影");
    assert_eq!(current.attempt, previous.attempt + 1);
    assert!(current.focus_epoch > previous.epoch);
    assert!(matches!(port.attach(previous), Err(Reject::StaleAttempt)));
    drop(root);
    let ended = query(&port, VERSION).lifecycle.expect("最终结局");
    assert_eq!(ended.state, NativeLifecycleState::Ended);
    assert_eq!(
        port.command(
            port.binding(),
            2,
            NativeCommand::Event(DesktopEvent::Reconnect)
        ),
        Err(Reject::Retired)
    );
    assert_eq!(query(&port, VERSION).lifecycle.expect("迟到回调"), ended);
    let request = DesktopNativeLifecycleQuery {
        version: VERSION,
        source: ended.source.clone(),
        identity: ended.identity.clone(),
        request_id: 9,
    };
    drop(surface);
    drop(port);
    {
        let _contention = tests::hold_source_registration();
        assert!(matches!(
            lookup(&request.source, &request.identity),
            Err(Reject::Busy)
        ));
        let response = query_lifecycle(request.clone());
        assert_eq!(response.failure, NativeLifecycleQueryFailure::Busy);
        assert!(response.lifecycle.is_none());
    }
    {
        let _isolation = tests::isolate_source_registration();
        assert!(matches!(
            lookup(&request.source, &request.identity),
            Err(Reject::StaleSource)
        ));
    }
    let retained = query_request(request.clone())
        .lifecycle
        .expect("晚订阅读取有界结局");
    assert_eq!(retained, ended);
    let profile =
        rshell_m0::rshell_core::TerminalSettingsV1::default().resolve(&Default::default());
    let replacement = InputPort::register(
        Source {
            source: DesktopSource {
                session_id: ended.source.session_id,
                generation: ended.source.generation + 1,
            },
            identity: "新来源".into(),
        },
        Arc::new(profile),
        Arc::default(),
    )
    .expect("新 generation");
    replacement.0.begin_attempt();
    assert_eq!(
        query_request(request).lifecycle.expect("原来源不迁移"),
        ended
    );
    let values: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    assert!(
        values
            .windows(2)
            .all(|pair| pair[0].lifecycle_revision < pair[1].lifecycle_revision)
    );
    assert_eq!(
        values.last().expect("最终事件").state,
        NativeLifecycleState::Ended
    );
}
