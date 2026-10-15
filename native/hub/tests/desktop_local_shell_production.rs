//! direct target 使用应用 production 同源码；外层监督只回收本轮原进程对象。
#![cfg(windows)]

#[path = "../src/local_shell/production.rs"]
mod production;

mod windows {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::windows::io::AsRawHandle;
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    use super::production::{
        AdmissionError, Completion, Event, Geometry, Hooks, Launch, LocalOwner, TreeOutcome,
        WriteOutcome,
    };
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetProcessId, GetProcessTimes};

    fn succeeds<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("预期成功：{error:?}"),
        }
    }
    fn fails<T, E>(result: Result<T, E>) -> E {
        match result {
            Err(error) => error,
            Ok(_) => panic!("预期失败却成功"),
        }
    }
    fn present<T>(value: Option<T>) -> T {
        match value {
            Some(value) => value,
            None => panic!("缺少预期的自有资源或结果"),
        }
    }
    fn temp_root() -> std::path::PathBuf {
        let root = std::env::temp_dir();
        assert!(
            succeeds(std::fs::metadata(&root)).is_dir(),
            "创建前必须确认临时父目录存在"
        );
        root
    }

    fn geometry(cols: u16, rows: u16) -> Geometry {
        Geometry {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    fn launch(script: &str) -> Launch {
        Launch {
            program: "pwsh.exe".into(),
            args: [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            cwd: Some(std::env::temp_dir()),
            env: BTreeMap::from([("GUO_CONTROLLED".into(), "owned-env".into())]),
        }
    }

    struct Marker {
        path: std::path::PathBuf,
        parent: std::path::PathBuf,
    }
    impl Marker {
        fn new() -> Self {
            let path = temp_root().join(format!("guo-local-{}.marker", uuid::Uuid::new_v4()));
            let parent = succeeds(std::fs::canonicalize(present(path.parent())));
            assert!(!path.exists());
            eprintln!("本轮 owned 临时文件：{}", path.display());
            succeeds(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path),
            );
            Self { path, parent }
        }
    }
    impl Drop for Marker {
        fn drop(&mut self) {
            assert_eq!(
                succeeds(std::fs::canonicalize(present(self.path.parent()))),
                self.parent
            );
            succeeds(std::fs::remove_file(&self.path));
            assert!(!self.path.exists());
        }
    }

    struct OwnedDirectory {
        path: std::path::PathBuf,
        parent: std::path::PathBuf,
    }
    impl OwnedDirectory {
        fn new() -> Self {
            let path = temp_root().join(format!("guo-local-cwd-目录🦀-{}", uuid::Uuid::new_v4()));
            let parent = succeeds(std::fs::canonicalize(present(path.parent())));
            eprintln!("本轮 owned 临时目录：{}", path.display());
            succeeds(std::fs::create_dir(&path));
            Self { path, parent }
        }
    }
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            assert_eq!(
                succeeds(std::fs::canonicalize(present(self.path.parent()))),
                self.parent
            );
            if self.path.exists() {
                succeeds(std::fs::remove_dir(&self.path));
            }
            assert!(!self.path.exists());
        }
    }

    fn start(script: &str) -> LocalOwner {
        succeeds(LocalOwner::start(
            launch(script),
            "guo-owned-term".into(),
            geometry(93, 31),
        ))
    }

    fn completion(owner: &LocalOwner, result: &Completion) {
        let state = owner.observation();
        assert!(state.cleanup_finished);
        assert_eq!(state.writer_joined, state.writer_started);
        assert_eq!(state.reader_joined, state.reader_started);
        assert_eq!(state.writer_dropped, state.writer_bound);
        assert_eq!(state.reader_dropped, state.reader_bound);
        assert_eq!(state.master_dropped, state.master_opened);
        if result.tree != TreeOutcome::NotSpawned {
            assert!(state.child_reaped);
            assert!(state.writer_joined_at < state.child_reaped_at);
            assert!(state.child_reaped_at < state.tree_dropped_at);
        }
        if state.master_opened {
            assert!(state.tree_dropped_at < state.master_dropped_at);
        }
        if state.reader_started {
            assert!(state.master_dropped_at < state.reader_joined_at);
        }
        assert_eq!(state.write_entered, state.write_returned);
        assert_eq!((state.input_bytes, state.input_items), (0, 0));
        assert!(
            result.cleanup_errors.is_empty(),
            "次级清理错误：{:?}",
            result.cleanup_errors
        );
        if let Some(identity) = owner.identity() {
            assert!(identity.pid != 0 && identity.started != 0);
            assert!(succeeds(identity.is_exited()));
            assert_eq!(result.tree, TreeOutcome::ContainedWindows);
        }
    }

    async fn until(owner: &mut LocalOwner, output: &mut Vec<u8>, needle: &[u8]) {
        while !output.windows(needle.len()).any(|bytes| bytes == needle) {
            match owner.next_event().await {
                Some(Event::Output(bytes)) => output.extend(bytes),
                other => panic!("尾输出之前意外结束：{other:?}"),
            }
            assert!(output.len() <= 256 * 1024);
        }
    }

    async fn roundtrip() {
        let marker = Marker::new();
        succeeds(std::fs::write(&marker.path, b"owned-cwd"));
        let mut config = launch(concat!(
            "[Console]::InputEncoding=[Text.UTF8Encoding]::new($false);",
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);",
            "[Console]::WriteLine(('READY:{0}:{1}:{2}x{3}' -f $env:TERM,$env:GUO_CONTROLLED,[Console]::WindowWidth,[Console]::WindowHeight));",
            "[Console]::WriteLine(('ENV:'+ [Environment]::GetEnvironmentVariable('变量🦀')));",
            "if([IO.File]::ReadAllText($env:GUO_CWD_MARKER) -ne 'owned-cwd'){exit 55};[Console]::WriteLine('CWD_OK');",
            "$line=[Console]::ReadLine();[Console]::WriteLine(('ECHO:'+$line));",
            "$null=[Console]::ReadLine();",
            "[Console]::WriteLine(('RESIZED:{0}x{1}' -f [Console]::WindowWidth,[Console]::WindowHeight));",
            "[Console]::WriteLine('TAIL:结束🦀');exit 23"
        ));
        config.env.insert(
            "GUO_CWD_MARKER".into(),
            present(marker.path.file_name()).into(),
        );
        config.env.insert("变量🦀".into(), "合法é值".into());
        config.env.insert("pathext".into(), ".EXE;.é".into());
        let mut owner = succeeds(LocalOwner::start(
            config,
            "guo-owned-term".into(),
            geometry(93, 31),
        ));
        succeeds(owner.ready().await);
        let identity = present(owner.identity());
        let mut output = Vec::new();
        until(
            &mut owner,
            &mut output,
            b"READY:guo-owned-term:owned-env:93x31",
        )
        .await;
        let payload = "你好é🦀\r\n".as_bytes();
        assert!(
            matches!(succeeds(succeeds(owner.try_write(payload)).finish().await), WriteOutcome::Complete(n) if n == payload.len())
        );
        until(&mut owner, &mut output, "ECHO:你好é🦀".as_bytes()).await;
        succeeds(owner.resize(geometry(107, 37)).await);
        succeeds(succeeds(owner.try_write(b"next\r\n")).finish().await);
        let result = loop {
            match owner.next_event().await {
                Some(Event::Output(bytes)) => output.extend(bytes),
                Some(Event::Closed(result)) => break result,
                None => panic!("未收到真实完成"),
            }
        };
        assert_eq!(result.exit_code, Some(23));
        assert!(!result.cancelled && result.primary.is_none());
        for needle in ["RESIZED:107x37", "TAIL:结束🦀"] {
            assert!(
                output
                    .windows(needle.len())
                    .any(|bytes| bytes == needle.as_bytes())
            );
        }
        assert!(
            output.windows(6).any(|bytes| bytes == b"CWD_OK"),
            "child 相对路径必须读取显式 cwd 中的本轮文件"
        );
        assert!(
            output
                .windows("ENV:合法é值".len())
                .any(|bytes| bytes == "ENV:合法é值".as_bytes())
        );
        completion(&owner, &result);
        assert!(owner.next_event().await.is_none());
        drop(owner);
        assert!(succeeds(identity.is_exited()));
    }

    async fn blocked() {
        let owner = start(concat!(
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);",
            "[Console]::WriteLine('BLOCKED');",
            "for($i=0;$i -lt 8192;$i++){[Console]::WriteLine(('z'*256))};",
            "[Threading.Thread]::Sleep(60000)"
        ));
        succeeds(owner.ready().await);
        let deadline = Instant::now() + Duration::from_secs(8);
        let bytes = [b'x'; 8192];
        let mut admitted = 0;
        let mut last_ticket = None;
        loop {
            let state = owner.observation();
            assert!(state.input_bytes <= 65536 && state.output_bytes <= 65536);
            assert!(state.input_items <= 8 && state.output_items <= 8);
            if state.output_items == 8
                && state.write_entered > state.write_returned
                && state.input_items == 8
            {
                break;
            }
            match owner.try_write(&bytes) {
                Ok(ticket) => {
                    last_ticket = Some(ticket);
                    admitted += bytes.len();
                }
                Err(AdmissionError::Capacity) => {}
                Err(error) => panic!("堵写前关闭：{error:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "未观测真实在途堵写，已准入 {admitted} bytes"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let before = owner.observation();
        // 同一 current-thread runtime 继续消费另一 owner，堵写不能阻塞异步事件面。
        let mut other = start(
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);[Console]::WriteLine('OTHER:独立🦀');exit 17",
        );
        succeeds(other.ready().await);
        let mut output = Vec::new();
        let other_result = loop {
            match other.next_event().await {
                Some(Event::Output(bytes)) => output.extend(bytes),
                Some(Event::Closed(result)) => break result,
                None => panic!("另一 owner 缺少完成"),
            }
        };
        assert_eq!(other_result.exit_code, Some(17));
        assert!(
            output
                .windows("OTHER:独立🦀".len())
                .any(|bytes| bytes == "OTHER:独立🦀".as_bytes())
        );
        completion(&other, &other_result);
        assert!(owner.observation().write_entered > owner.observation().write_returned);
        owner.cancel();
        assert!(matches!(
            owner.try_write(b"late"),
            Err(AdmissionError::Closing)
        ));
        let result = owner.close().await;
        match succeeds(present(last_ticket).finish().await) {
            WriteOutcome::Closing { committed, error } => {
                assert!(committed <= bytes.len());
                if let Some(error) = error {
                    assert_eq!(error.operation, "write");
                }
            }
            WriteOutcome::Failed { committed, error } => panic!(
                "取消不能改写为 write 成功或独立失败：committed={committed}, error={error:?}"
            ),
            other => panic!("排队中的写必须保留 Closing：{other:?}"),
        }
        completion(&owner, &result);
        assert!(result.cancelled && result.primary.is_none());
        eprintln!(
            "真实满队列+堵写：admitted={admitted}，取消前 entered={} returned={}；worker 已全部 join",
            before.write_entered, before.write_returned
        );
    }

    async fn partial_and_late() {
        let partial = succeeds(LocalOwner::start_test(
            launch("[Console]::WriteLine(('z'*8192));[Threading.Thread]::Sleep(60000)"),
            "guo-owned-term".into(),
            geometry(93, 31),
            Hooks {
                fail_after_reader: true,
                ..Hooks::default()
            },
        ));
        assert_eq!(fails(partial.ready().await).operation, "partial-init");
        let (result, duplicate) = tokio::join!(partial.close(), partial.close());
        assert_eq!(result.exit_code, duplicate.exit_code);
        assert_eq!(present(result.primary.as_ref()).operation, "partial-init");
        completion(&partial, &result);
        let barrier = Arc::new(Barrier::new(2));
        let cwd = OwnedDirectory::new();
        let mut late_launch = launch("[Threading.Thread]::Sleep(60000)");
        late_launch.cwd = Some(cwd.path.clone());
        let late = succeeds(LocalOwner::start_test(
            late_launch,
            "guo-owned-term".into(),
            geometry(93, 31),
            Hooks {
                before_spawn: Some(Arc::clone(&barrier)),
                ..Hooks::default()
            },
        ));
        barrier.wait();
        let delete_while_launching = std::fs::remove_dir(&cwd.path);
        late.cancel();
        barrier.wait();
        assert!(
            delete_while_launching.is_err(),
            "spawn 期间不能让显式 cwd 被删除而触发上游回退"
        );
        assert!(late.ready().await.is_err());
        assert!(
            late.identity().is_some(),
            "不可取消的晚返回 spawn 必须仍然接管 child"
        );
        let result = late.close().await;
        completion(&late, &result);
        drop(cwd);
        let mut invalid = launch("exit 0");
        invalid.cwd =
            Some(std::env::temp_dir().join(format!("guo-absent-{}", uuid::Uuid::new_v4())));
        let invalid = succeeds(LocalOwner::start(
            invalid,
            "guo-owned-term".into(),
            geometry(93, 31),
        ));
        assert_eq!(fails(invalid.ready().await).operation, "cwd");
        assert_eq!(
            present(
                fails(invalid.ready().await)
                    .source
                    .downcast_ref::<std::io::Error>()
            )
            .kind(),
            std::io::ErrorKind::NotFound
        );
        assert!(invalid.identity().is_none());
        let result = invalid.close().await;
        completion(&invalid, &result);
        assert_eq!(result.tree, TreeOutcome::NotSpawned);
    }

    async fn tree_and_drop() {
        let mut owner = start(
            "$p=Start-Process pwsh.exe -ArgumentList '-NoLogo','-NoProfile','-NonInteractive','-Command','[Threading.Thread]::Sleep(60000)' -PassThru -ErrorAction Stop;if($p.HasExited){exit 55};[Console]::WriteLine('TREE_READY');[Threading.Thread]::Sleep(60000)",
        );
        succeeds(owner.ready().await);
        let identity = present(owner.identity());
        let observer = owner.test_observer();
        let mut output = Vec::new();
        until(&mut owner, &mut output, b"TREE_READY").await;
        // 此处故意只用 Drop，检验最后责任根确实完成，而不是后台 detach。
        drop(owner);
        assert!(succeeds(identity.is_exited()));
        let state = observer();
        assert!(
            state.cleanup_finished
                && state.child_reaped
                && state.writer_joined
                && state.reader_joined
                && state.controller_joined
        );
    }

    async fn unsafe_pathext() {
        use std::os::windows::ffi::OsStringExt;
        for value in [
            OsString::from("é"),
            OsString::from(".EXE;;.CMD"),
            OsString::from(".bad/extension"),
            OsString::from_wide(&[0xd800]),
        ] {
            let mut config = launch("exit 0");
            config.program = format!("guo-absent-{}", uuid::Uuid::new_v4()).into();
            config
                .env
                .insert("PATH".into(), temp_root().into_os_string());
            config.env.insert("pAtHeXt".into(), value);
            let owner = match LocalOwner::start(config, "guo-owned-term".into(), geometry(93, 31)) {
                Ok(owner) => owner,
                Err(error) => panic!("需要观测 controller 内有效环境校验：{error:?}"),
            };
            let observer = owner.test_observer();
            let error = fails(succeeds(
                tokio::time::timeout(Duration::from_secs(1), owner.ready()).await,
            ));
            let result = owner.close().await;
            assert!(Arc::ptr_eq(
                &error.source,
                &present(result.primary.as_ref()).source
            ));
            let before = owner.observation();
            drop(owner);
            let after = observer();
            eprintln!("PATHEXT 实际边界：before={before:?}, after={after:?}");
            assert!(after.cleanup_finished && after.controller_joined);
            assert_eq!(error.operation, "launch-pathext");
        }
    }

    async fn initialization_settlement() {
        use std::future::Future;
        use std::task::Poll;
        for stage in ["job", "reader-worker", "writer-worker"] {
            let barrier = Arc::new(Barrier::new(2));
            let owner = match LocalOwner::start_test(
                launch("[Threading.Thread]::Sleep(60000)"),
                "guo-owned-term".into(),
                geometry(93, 31),
                Hooks {
                    before_initialize: Some(Arc::clone(&barrier)),
                    fail_tree: stage == "job",
                    fail_before_workers: stage == "reader-worker",
                    fail_after_reader_worker: stage == "writer-worker",
                    ..Hooks::default()
                },
            ) {
                Ok(owner) => owner,
                Err(error) => panic!("start: {error:?}"),
            };
            barrier.wait();
            let ticket = match owner.try_write(b"queued") {
                Ok(ticket) => ticket,
                Err(error) => panic!("提前准入：{error:?}"),
            };
            let mut ready = Box::pin(owner.ready());
            std::future::poll_fn(|cx| {
                assert!(ready.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(ready);
            let mut resize = Box::pin(owner.resize(geometry(107, 37)));
            std::future::poll_fn(|cx| {
                assert!(resize.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            barrier.wait();
            let error = match owner.ready().await {
                Err(error) => error,
                Ok(()) => panic!("初始化故障未交付"),
            };
            assert_eq!(error.operation, stage);
            let result = owner.close().await;
            eprintln!("初始化结算：{stage}, {:?}", owner.observation());
            assert!(matches!(
                owner.try_write(b"late"),
                Err(AdmissionError::Closing)
            ));
            assert_eq!(
                (
                    owner.observation().input_bytes,
                    owner.observation().input_items
                ),
                (0, 0)
            );
            assert_eq!(
                result.primary.as_ref().map(|error| error.operation),
                Some(stage)
            );
            assert!(matches!(
                ticket.finish().await,
                Ok(WriteOutcome::Closing { committed: 0, .. })
            ));
            assert!(resize.await.is_err());
            assert!(owner.resize(geometry(93, 31)).await.is_err());
            completion(&owner, &result);
            let observer = owner.test_observer();
            drop(owner);
            assert!(observer().controller_joined);
        }
    }

    async fn termination_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for count in [1, 3] {
            let failures = Arc::new(AtomicUsize::new(count));
            let owner = succeeds(LocalOwner::start_test(
                launch("[Threading.Thread]::Sleep(60000)"),
                "guo-owned-term".into(),
                geometry(93, 31),
                Hooks {
                    terminate_failures: Some(Arc::clone(&failures)),
                    fail_after_reader: true,
                    ..Hooks::default()
                },
            ));
            let error = fails(succeeds(
                tokio::time::timeout(Duration::from_secs(2), owner.ready()).await,
            ));
            assert_eq!(error.operation, "partial-init");
            let result = owner.close().await;
            assert_eq!(owner.observation().terminate_failures, count);
            assert!(owner.observation().terminate_attempts > count);
            assert!(
                owner.observation().child_reaped
                    && owner.observation().reader_joined
                    && owner.observation().writer_joined
            );
            assert_eq!(result.tree, TreeOutcome::ContainedWindows);
            assert!(succeeds(present(owner.identity()).is_exited()));
            let primary = present(result.primary.as_ref());
            assert!(
                Arc::ptr_eq(&error.source, &primary.source),
                "清理次错不能替换原 primary 对象"
            );
            eprintln!(
                "有限终止故障 {count} 次后真实收敛：{:?}",
                owner.observation()
            );
        }
        let failures = Arc::new(AtomicUsize::new(usize::MAX));
        let owner = match LocalOwner::start_test(
            launch("[Threading.Thread]::Sleep(60000)"),
            "guo-owned-term".into(),
            geometry(93, 31),
            Hooks {
                terminate_failures: Some(Arc::clone(&failures)),
                fail_after_reader: true,
                ..Hooks::default()
            },
        ) {
            Ok(owner) => owner,
            Err(error) => panic!("start: {error:?}"),
        };
        let deadline = Instant::now() + Duration::from_secs(1);
        while owner.identity().is_none() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let identity = match owner.identity() {
            Some(identity) => identity,
            None => panic!("缺少原 child 身份"),
        };
        let pending = tokio::time::timeout(Duration::from_millis(150), owner.close()).await;
        eprintln!(
            "失败边界 pending={pending:?}, remaining={}, observation={:?}",
            failures.load(Ordering::Acquire),
            owner.observation()
        );
        assert!(pending.is_err());
        assert!(matches!(identity.is_exited(), Ok(false)));
        assert!(!owner.observation().cleanup_finished && !owner.observation().child_reaped);
        let status = owner.closing_status();
        assert!(status.pending);
        assert_eq!(
            status.primary.as_ref().map(|error| error.operation),
            Some("partial-init")
        );
        assert_eq!(
            status
                .last_cleanup_error
                .as_ref()
                .map(|error| error.operation),
            Some("job-terminate")
        );
        assert!(
            owner.observation().terminate_attempts > 1
                && owner.observation().terminate_failures > 1
        );
        eprintln!(
            "终止失败仍持根：pid={} started={} fault_remaining={}",
            identity.pid,
            identity.started,
            failures.load(Ordering::Acquire)
        );
        failures.store(0, Ordering::Release);
        let result = match tokio::time::timeout(Duration::from_secs(1), owner.close()).await {
            Ok(result) => result,
            Err(error) => panic!("恢复后必须重试自有 Job 而不是卡在 INFINITE wait：{error}"),
        };
        assert!(
            owner.observation().child_reaped
                && owner.observation().reader_joined
                && owner.observation().writer_joined
        );
        assert!(matches!(identity.is_exited(), Ok(true)));
        assert!(
            result
                .cleanup_errors
                .iter()
                .any(|error| error.operation == "job-terminate")
        );
        assert_eq!(
            result.primary.as_ref().map(|error| error.operation),
            Some("partial-init")
        );
        assert!(!owner.closing_status().pending);
    }

    async fn controller_unwind() {
        let owner = match LocalOwner::start_test(
            launch("[Threading.Thread]::Sleep(60000)"),
            "guo-owned-term".into(),
            geometry(93, 31),
            Hooks {
                library_panic_after_reader: Some(
                    format!("guo-absent-panic-{}", uuid::Uuid::new_v4()).into(),
                ),
                ..Hooks::default()
            },
        ) {
            Ok(owner) => owner,
            Err(error) => panic!("start: {error:?}"),
        };
        let error = match tokio::time::timeout(Duration::from_secs(2), owner.ready()).await {
            Ok(Err(error)) => error,
            other => panic!("上游真实 panic 必须完成清理并交付错误：{other:?}"),
        };
        assert_eq!(error.operation, "controller-panic");
        let result = owner.close().await;
        assert_eq!(
            result.primary.as_ref().map(|error| error.operation),
            Some("controller-panic")
        );
        assert!(
            owner.identity().is_some(),
            "必须实际拥有 child 才能证明 unwind 清理"
        );
        completion(&owner, &result);
        let observer = owner.test_observer();
        drop(owner);
        assert!(observer().controller_joined);
        eprintln!("真实上游 panic 后完整收口：{:?}", observer());
    }

    async fn abandoned_ready() {
        use std::future::Future;
        use std::task::Poll;
        let barrier = Arc::new(Barrier::new(2));
        let owner = match LocalOwner::start_test(
            launch("[Threading.Thread]::Sleep(60000)"),
            "guo-owned-term".into(),
            geometry(93, 31),
            Hooks {
                before_initialize: Some(Arc::clone(&barrier)),
                ..Hooks::default()
            },
        ) {
            Ok(owner) => owner,
            Err(error) => panic!("start: {error:?}"),
        };
        barrier.wait();
        let mut ready = Box::pin(owner.ready());
        std::future::poll_fn(|cx| {
            assert!(ready.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(ready);
        barrier.wait();
        match owner.ready().await {
            Ok(()) => {}
            Err(error) => panic!("ready: {error:?}"),
        }
        assert!(owner.identity().is_some());
        let result = owner.close().await;
        completion(&owner, &result);
        let observer = owner.test_observer();
        drop(owner);
        assert!(observer().controller_joined);
    }

    fn supervised(case: &str) {
        let mut child = succeeds(
            Command::new(succeeds(std::env::current_exe()))
                .args(["--exact", "windows::production_fixture", "--nocapture"])
                .env("GUO_LOCAL_PRODUCTION_CASE", case)
                .spawn(),
        );
        let raw = child.as_raw_handle();
        let pid = unsafe { GetProcessId(raw) };
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        assert_ne!(
            unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) },
            0
        );
        let started = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        assert!(pid != 0 && started != 0);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = succeeds(child.try_wait()) {
                assert!(status.success(), "真实 owner fixture 失败：{case}");
                break;
            }
            if Instant::now() >= deadline {
                assert_eq!(unsafe { GetProcessId(raw) }, pid);
                // std Child 保持原对象句柄；绝不重新按 PID 打开或杀未知进程。
                succeeds(child.kill());
                succeeds(child.wait());
                panic!("监督强制回收；本用例不通过：{case} start={started}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn owned_pty_unicode_geometry_and_natural_tail() {
        supervised("roundtrip");
    }
    #[test]
    fn bounded_queues_and_real_inflight_write_cancel() {
        supervised("blocked");
    }
    #[test]
    fn partial_initialization_and_late_spawn_remain_owned() {
        supervised("partial");
    }
    #[test]
    fn contained_descendants_and_drop_fallback() {
        supervised("tree");
    }
    #[test]
    fn unsafe_pathext_delivers_failure_after_cleanup() {
        supervised("pathext");
    }
    #[test]
    fn initialization_failure_settles_all_accepted_work() {
        supervised("settlement");
    }
    #[test]
    fn termination_failure_retains_root_and_retries_owned_job() {
        supervised("retry");
    }
    #[test]
    fn controller_unwind_reclaims_actual_owned_child() {
        supervised("unwind");
    }
    #[test]
    fn polled_ready_can_be_abandoned_without_abandoning_owner() {
        supervised("abandoned");
    }
    #[test]
    fn invalid_windows_geometry_is_rejected_without_spawn() {
        assert!(LocalOwner::start(launch("exit 0"), "xterm".into(), geometry(32768, 24)).is_err());
        assert!(LocalOwner::start(launch("exit 0"), "xterm".into(), geometry(80, 0)).is_err());
    }
    #[test]
    fn production_fixture() {
        let Ok(case) = std::env::var("GUO_LOCAL_PRODUCTION_CASE") else {
            return;
        };
        let runtime = succeeds(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
        );
        runtime.block_on(async {
            match case.as_str() {
                "roundtrip" => roundtrip().await,
                "blocked" => blocked().await,
                "partial" => partial_and_late().await,
                "tree" => tree_and_drop().await,
                "pathext" => unsafe_pathext().await,
                "settlement" => initialization_settlement().await,
                "retry" => termination_retry().await,
                "unwind" => controller_unwind().await,
                "abandoned" => abandoned_ready().await,
                _ => panic!("未知监督用例"),
            }
        });
    }
}
