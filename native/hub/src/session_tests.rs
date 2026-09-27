//! 会话连接生命周期回归；所有凭证均为内存替身，不访问真实钥匙串或硬件。

#![allow(clippy::expect_used)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rshell_m0::rshell_core::{
    AuthenticationKind, CatalogMutation, ConnectionProfile, CredentialRef, SecretUpdate,
    TerminalSize, TransportKind,
};
use rshell_m0::rshell_storage::{
    CredentialCoordinator, CredentialVault, MemoryCredentialVault, SqliteRepository, VaultError,
};
use secrecy::SecretString;
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::{SessionCommand, run_session};
use crate::app::AppContext;
use crate::card::{CardContext, NoCards};
use crate::connect::{self, Abort, Channels, PromptIds, Target};
use crate::keys::{MemoryKeyStore, PreferenceFile};
use crate::signals::ConnectRequest;
use crate::signals::interaction::InteractionReply;

fn context(vault: Arc<dyn CredentialVault>) -> Arc<AppContext> {
    let repository = Arc::new(SqliteRepository::open_in_memory().expect("内存目录"));
    repository.migrate().expect("目录迁移");
    Arc::new(AppContext {
        credentials: CredentialCoordinator::new(repository.clone(), vault),
        repository,
        known_hosts: PathBuf::new(),
        keys: Arc::new(MemoryKeyStore::new(false)),
        key_operations: Mutex::new(()),
        cards: Arc::new(CardContext::new(Arc::new(NoCards))),
        security_keys: Arc::new(crate::security_key::Unavailable),
        preferences: PreferenceFile::open(
            std::env::temp_dir().join(format!("guosh-session-tests-{}.json", uuid::Uuid::new_v4())),
        ),
        catalog_changed: Notify::new(),
        keys_changed: Notify::new(),
    })
}

fn profile() -> ConnectionProfile {
    let mut profile = ConnectionProfile::new("会话回归", "example.test");
    // 目录允许空用户名，NativeSshTransport 在建网前拒绝；已回答的连接只走到校验。
    profile.username.clear();
    profile.authentication = AuthenticationKind::Password;
    profile.transport = TransportKind::NativeSsh;
    profile
}

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    }
}

fn reply(prompt_id: u32, accept: bool) -> InteractionReply {
    InteractionReply {
        session_id: 1,
        prompt_id,
        accept,
        answers: vec!["测试口令".into()],
        remember: false,
    }
}

#[tokio::test]
async fn stale_acceptance_and_rejection_cannot_answer_a_reconnect_prompt() {
    for stale_accept in [true, false] {
        let context = context(Arc::new(MemoryCredentialVault::new()));
        let (commands_tx, mut commands) = unbounded_channel();
        let (replies_tx, mut replies) = unbounded_channel();
        let mut backlog = VecDeque::new();
        let mut prompts = PromptIds::default();
        let target = || Target {
            profile: profile(),
            saved: false,
        };
        assert!(replies_tx.send(reply(1, false)).is_ok());
        let first = connect::establish(
            &context,
            1,
            target(),
            SecretString::from(String::new()),
            size(),
            Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            },
            &mut prompts,
        )
        .await;
        assert!(matches!(first, Err(Abort::Cancelled)));

        // 模拟上一轮关闭后到达的旧答案排在本轮答案前面。
        assert!(replies_tx.send(reply(1, stale_accept)).is_ok());
        assert!(replies_tx.send(reply(2, !stale_accept)).is_ok());
        let second = connect::establish(
            &context,
            1,
            target(),
            SecretString::from(String::new()),
            size(),
            Channels {
                commands: &mut commands,
                replies: &mut replies,
                backlog: &mut backlog,
            },
            &mut prompts,
        )
        .await;
        if stale_accept {
            assert!(matches!(second, Err(Abort::Cancelled)), "只能接受本轮拒绝");
        } else {
            assert!(
                matches!(second, Err(Abort::Failed { .. })),
                "本轮回答应进入 transport 校验"
            );
        }
        drop(commands_tx);
    }
}

/// 每次读密码都通知测试，便于精确进入重连中的交互等待。
struct NotifyingVault(UnboundedSender<()>);

impl CredentialVault for NotifyingVault {
    fn get(&self, _: &CredentialRef) -> Result<Option<SecretString>, VaultError> {
        let _ = self.0.send(());
        Ok(None)
    }

    fn put(&self, _: &CredentialRef, _: &SecretString) -> Result<(), VaultError> {
        // 测试建立目录引用后模拟凭证缺失，使每轮都走连接端的交互分支。
        Ok(())
    }

    fn delete(&self, _: &CredentialRef) -> Result<(), VaultError> {
        unreachable!("此测试不删除凭证")
    }
}

#[tokio::test]
async fn closing_a_pane_during_reconnect_releases_the_session_and_queues() {
    let (reads_tx, mut reads) = unbounded_channel();
    let context = context(Arc::new(NotifyingVault(reads_tx)));
    let profile = profile();
    let id = profile.id.0.to_string();
    context
        .credentials
        .apply_catalog(
            CatalogMutation::Create(profile),
            SecretUpdate::Set(SecretString::from("测试凭证".to_owned())),
        )
        .expect("测试连接");
    let request = ConnectRequest {
        session_id: 1,
        connection_id: id,
        host: String::new(),
        port: 22,
        username: String::new(),
        // 第一轮无需提问，并确定以 transport 配置失败结束。
        password: "首轮测试口令".into(),
        command: String::new(),
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 96,
    };
    let (commands_tx, commands) = unbounded_channel();
    let (replies_tx, replies) = unbounded_channel();
    let released = Arc::downgrade(&context);
    let task = tokio::spawn(run_session(context, request, commands, replies));
    tokio::time::timeout(Duration::from_secs(2), reads.recv())
        .await
        .expect("第一轮")
        .expect("读密码");
    assert!(commands_tx.send(SessionCommand::Reconnect).is_ok());
    tokio::time::timeout(Duration::from_secs(2), reads.recv())
        .await
        .expect("重连开始")
        .expect("读密码");
    assert!(commands_tx.send(SessionCommand::Disconnect).is_ok());
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("关闭窗格后会话必须结束")
        .expect("会话任务");
    assert!(commands_tx.is_closed());
    assert!(replies_tx.is_closed());
    assert!(released.upgrade().is_none(), "会话不再持有共享缓存");
}
