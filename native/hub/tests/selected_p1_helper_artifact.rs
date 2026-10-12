//! 实际 MSVC candidate → 原生 Pool → raw → 完整领域快照；不注册生产入口。

#[path = "../src/selected_p1/helper_artifact/test_audit.rs"]
pub mod audit;
#[path = "../src/selected_p1/model/capacity.rs"]
pub mod capacity;
#[path = "../src/selected_p1/model/certificate.rs"]
pub mod certificate;
#[path = "../src/selected_p1/model/layout.rs"]
pub mod layout;
#[path = "../src/selected_p1/resource/ledger.rs"]
pub mod ledger;
#[path = "../src/selected_p1/resource/owner.rs"]
pub mod owner;
#[path = "../src/selected_p1/resource/secret.rs"]
pub mod secret;
#[global_allocator]
static ALLOCATOR: audit::Allocator = audit::Allocator;
#[path = "../src/selected_p1/decode_shape/mod.rs"]
pub mod decode_shape;
#[path = "../src/selected_p1/domain_decode/mod.rs"]
pub mod domain_decode;
#[cfg(windows)]
#[path = "../src/selected_p1/helper_artifact/mod.rs"]
pub mod helper_artifact;
#[path = "../src/selected_p1/helper_runtime/process.rs"]
pub mod process;
#[cfg(windows)]
#[path = "../src/selected_p1/helper_runtime/windows.rs"]
pub mod windows;
#[path = "../src/selected_p1/helper_runtime/worker.rs"]
pub mod worker;

pub mod selected_p1 {
    pub use crate::{decode_shape, domain_decode};
    pub mod model {
        pub use crate::{capacity, certificate, layout};
    }
    pub mod resource {
        pub use crate::{ledger, owner, secret};
    }
    pub mod helper_runtime {
        pub use crate::{process, worker};
    }
}

#[cfg(windows)]
mod artifact_tests {
    use crate::{
        audit, certificate,
        domain_decode::{self, ExecutionCounts, raw::Selection},
        helper_artifact::{self, Inputs, OwnedRecord, Reject},
        ledger::{Admission, BYTE_LIMIT, CLASS_COUNT, ProcessLedger, ResourceClass},
        process::{Failure, GroupReferenceFact, LaunchIdentity, Request},
        windows::{SpawnCostPlan, TrustedLaunch},
        worker::{Completion, HelperPool, Submission, Ticket},
    };
    use rshell_m0::{
        rshell_core::{
            AppSettings, AuthenticationKind, CatalogMutation, ConnectionGroup, ConnectionId,
            ConnectionProfile, GroupId, KeyBinding, KeyCode, KeyModifiers, TerminalProfile,
            TerminalProfileId, TerminalSettingsV1, TransportKind,
        },
        rshell_storage::SqliteRepository,
    };
    use std::{
        ffi::{CString, OsStr, c_char, c_int, c_void},
        path::PathBuf,
        time::{Duration, Instant},
    };
    use uuid::Uuid;

    const ADMISSION: Admission = Admission {
        epoch: 1,
        source: None,
    };
    static ROOT: ProcessLedger = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);

    struct OwnedDirectory(PathBuf);
    impl std::ops::Deref for OwnedDirectory {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            assert!(
                self.0
                    .parent()
                    .is_some_and(|parent| parent == std::env::temp_dir().join("opencode"))
            );
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                if std::thread::panicking() {
                    eprintln!("合成 fixture 清理失败: {:?}", error.kind());
                } else {
                    panic!("合成 fixture 清理失败: {:?}", error.kind());
                }
            }
        }
    }

    struct Fixture {
        repository: SqliteRepository,
        directory: OwnedDirectory,
        database: PathBuf,
        candidate: PathBuf,
        record: PathBuf,
        recipe: PathBuf,
        sources: PathBuf,
        connection: ConnectionId,
        profile: TerminalProfileId,
    }
    impl Fixture {
        fn new() -> Self {
            let candidate = PathBuf::from(
                std::env::var_os("SELECTED_HELPER_ARTIFACT_DIR")
                    .unwrap_or_else(|| panic!("需要父指定的真实候选目录")),
            );
            let record = candidate.join("artifact_record.json");
            let recipe = candidate.join("build_recipe.json");
            // 仅测试 setup 提取真实记录的路径；生产 load 自行 bounded 验证全部记录。
            let json: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&record).unwrap_or_else(|_| panic!("读取真实候选记录")),
            )
            .unwrap_or_else(|_| panic!("候选 JSON"));
            let sources = PathBuf::from(
                json["upstreamRoot"]
                    .as_str()
                    .unwrap_or_else(|| panic!("来源根")),
            );
            let executable = PathBuf::from(
                json["artifact"]["path"]
                    .as_str()
                    .unwrap_or_else(|| panic!("真实映像")),
            );
            assert_eq!(
                std::fs::canonicalize(&executable).unwrap_or_else(|_| panic!("实际映像")),
                std::fs::canonicalize(
                    candidate.join("cargo/x86_64-pc-windows-msvc/debug/selected-read-helper.exe")
                )
                .unwrap_or_else(|_| panic!("指定候选映像"))
            );
            let parent = std::env::temp_dir().join("opencode");
            assert!(parent.is_dir());
            let directory = parent.join(format!("selected-artifact-{}", Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap_or_else(|_| panic!("合成目录"));
            // 从创建起拥有 cleanup，setup 失败也不遗留合成 DB。
            let directory = OwnedDirectory(directory);
            let database = directory.join("合成 数据🦀.db");
            let repository =
                SqliteRepository::open(&database).unwrap_or_else(|error| panic!("{error:?}"));
            repository
                .migrate()
                .unwrap_or_else(|error| panic!("{error:?}"));
            let profile = TerminalProfileId(Uuid::from_u128(2));
            let settings = TerminalSettingsV1 {
                terminal_type: " xterm-custom ".into(),
                font_family: "完整字体".into(), answerback: "原始\0🦀".into(),
                key_bindings: vec![KeyBinding { code: KeyCode::Enter, modifiers: KeyModifiers::default(), action: "new_tab".into() }],
                extensions: serde_json::from_value(serde_json::json!({"opaque": {"nested": [true, null, "原始\u{0}🦀", {"value": [1, 2.5]}]}, "future": {"version": 91}})).unwrap_or_else(|error| panic!("{error}")),
                ..TerminalSettingsV1::default()
            };
            repository
                .save_terminal_profile(TerminalProfile {
                    id: profile,
                    name: "实际所选".into(),
                    settings,
                })
                .unwrap_or_else(|error| panic!("{error:?}"));
            repository
                .save_settings(AppSettings {
                    default_terminal_profile: profile,
                    key_bindings: vec![KeyBinding {
                        code: KeyCode::F(2),
                        modifiers: KeyModifiers::default(),
                        action: "split_vertical".into(),
                    }],
                    ..AppSettings::default()
                })
                .unwrap_or_else(|error| panic!("{error:?}"));
            let mut connection = ConnectionProfile::new("合成已保存", "fixture.invalid");
            connection.id = ConnectionId(Uuid::from_u128(3));
            connection.username = "fixture".into();
            connection.transport = TransportKind::NativeSsh;
            connection.authentication = AuthenticationKind::Password;
            connection.terminal_profile_id = Some(profile);
            connection.note = "不投 Dart 的完整 note\0🦀".into();
            connection.identity_file = Some(PathBuf::from("opaque\0🦀"));
            connection.tags.insert("a".into());
            connection.tags.insert("中文🦀".into());
            let id = connection.id;
            repository
                .apply(CatalogMutation::Create(connection))
                .unwrap_or_else(|error| panic!("{error:?}"));
            Self {
                repository,
                directory,
                database,
                candidate: executable,
                record,
                recipe,
                sources,
                connection: id,
                profile,
            }
        }
        fn inputs(&self) -> Inputs<'_> {
            Inputs {
                record: &self.record,
                recipe: &self.recipe,
                source_root: &self.sources,
                executable: self.candidate.as_os_str(),
                database: self.database.as_os_str(),
            }
        }
        fn selection(&self, explicit: bool) -> Selection<'_> {
            Selection {
                connection: Some(self.connection),
                explicit_profile: explicit.then_some(self.profile),
                expected_transport: TransportKind::NativeSsh,
                quick: None,
            }
        }
        fn update(&self, sql: &std::ffi::CStr, value: &str) {
            update(&self.database, sql, value);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.repository
                .shutdown()
                .unwrap_or_else(|error| panic!("{error:?}"));
        }
    }

    unsafe extern "C" {
        fn sqlite3_open_v2(
            path: *const c_char,
            db: *mut *mut c_void,
            flags: c_int,
            vfs: *const c_char,
        ) -> c_int;
        fn sqlite3_prepare_v2(
            db: *mut c_void,
            sql: *const c_char,
            len: c_int,
            stmt: *mut *mut c_void,
            tail: *mut *const c_char,
        ) -> c_int;
        fn sqlite3_bind_text(
            stmt: *mut c_void,
            index: c_int,
            value: *const c_char,
            len: c_int,
            destructor: Option<unsafe extern "C" fn(*mut c_void)>,
        ) -> c_int;
        fn sqlite3_step(stmt: *mut c_void) -> c_int;
        fn sqlite3_finalize(stmt: *mut c_void) -> c_int;
        fn sqlite3_close(db: *mut c_void) -> c_int;
    }
    fn update(path: &std::path::Path, sql: &std::ffi::CStr, value: &str) {
        let path = CString::new(path.to_str().unwrap_or_else(|| panic!("fixture UTF8")))
            .unwrap_or_else(|_| panic!("fixture NUL"));
        let mut db = std::ptr::null_mut();
        let mut stmt = std::ptr::null_mut();
        // SAFETY: 仅本轮合成 DB，借用 value 保到 step/finalize，handle 独占并关闭。
        unsafe {
            assert_eq!(
                sqlite3_open_v2(path.as_ptr(), &mut db, 2, std::ptr::null()),
                0
            );
            // 仅这个合成注入连接关闭 FK，允许测试悬空/非法引用；repository 不变。
            assert_eq!(
                sqlite3_prepare_v2(
                    db,
                    c"PRAGMA foreign_keys=OFF".as_ptr(),
                    -1,
                    &mut stmt,
                    std::ptr::null_mut()
                ),
                0
            );
            let pragma_step = sqlite3_step(stmt);
            let pragma_finalize = sqlite3_finalize(stmt);
            assert_eq!((pragma_step, pragma_finalize), (101, 0));
            assert_eq!(
                sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()),
                0
            );
            assert_eq!(
                sqlite3_bind_text(
                    stmt,
                    1,
                    value.as_ptr().cast(),
                    c_int::try_from(value.len()).unwrap_or_else(|_| panic!("fixture 长度")),
                    None
                ),
                0
            );
            let step = sqlite3_step(stmt);
            let finalize = sqlite3_finalize(stmt);
            let close = sqlite3_close(db);
            assert_eq!((step, finalize, close), (101, 0, 0));
        }
    }
    fn request(fixture: &Fixture, helper: usize, explicit: bool) -> Request {
        let mut bytes = [0; 112];
        bytes[..8].copy_from_slice(helper_artifact::REQUEST_MAGIC);
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&(if explicit { 3u16 } else { 1u16 }).to_le_bytes());
        bytes[12..20].copy_from_slice(&(8u64 * 1024 * 1024).to_le_bytes());
        bytes[20..28].copy_from_slice(&65536u64.to_le_bytes());
        bytes[28..36].copy_from_slice(&1048576u64.to_le_bytes());
        bytes[36..40].copy_from_slice(&16u32.to_le_bytes());
        bytes[40..56].copy_from_slice(fixture.connection.0.as_bytes());
        if explicit {
            bytes[56..72].copy_from_slice(fixture.profile.0.as_bytes());
        }
        bytes[72..80].copy_from_slice(&(64u64 + 24 * 38 + 1048576 + 138).to_le_bytes());
        bytes[80..88].copy_from_slice(&(helper as u64).to_le_bytes());
        bytes[88..92].copy_from_slice(&38u32.to_le_bytes());
        bytes[96..].copy_from_slice(&[7; 16]);
        Request::parse(bytes, helper_artifact::REQUEST_MAGIC)
            .unwrap_or_else(|error| panic!("{error:?}"))
    }
    fn wait(pool: &HelperPool, ticket: Ticket) -> Completion {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(completion) = pool
                .take_result(ticket)
                .unwrap_or_else(|error| panic!("{error:?}"))
            {
                return completion;
            }
            assert!(Instant::now() < deadline, "真实 helper 收口 deadline");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn submit(
        pool: &HelperPool,
        fixture: &Fixture,
        record: &OwnedRecord<'_>,
        explicit: bool,
    ) -> (Ticket, process_cost::Charge) {
        let args = [record.database()];
        let identity = LaunchIdentity {
            executable: record.executable(),
            arguments: &args,
        };
        let shape = helper_artifact::model::launch_shape(identity)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let costs = record
            .helper_costs(identity, &shape)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let request = request(
            fixture,
            costs.total().unwrap_or_else(|error| panic!("{error:?}")),
            explicit,
        );
        let launch = unsafe { TrustedLaunch::borrowed(record.executable(), &args) }
            .unwrap_or_else(|error| panic!("{error:?}"));
        let plan = SpawnCostPlan::bound(&launch, &request, record)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let charge = process_cost::Charge {
            helper: plan.costs().helper.rust_bytes,
            c: plan.costs().helper.c_bytes,
            spawn: plan.costs().spawn.rust_bytes,
            raw: plan.costs().raw.rust_bytes,
        };
        let reserved = plan
            .costs()
            .reserve(&ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let ticket = pool
            .submit_reserved(
                Submission {
                    launch: &launch,
                    bytes: request.bytes,
                    record,
                    admission: ADMISSION,
                    timeout_ms: 10000,
                },
                reserved,
            )
            .unwrap_or_else(|error| panic!("{error:?}"));
        (ticket, charge)
    }
    mod process_cost {
        pub struct Charge {
            pub helper: usize,
            pub c: usize,
            pub spawn: usize,
            pub raw: usize,
        }
    }

    #[test]
    fn artifact_normal_candidate_full_snapshot_and_last_owner_audit() {
        let fixture = Fixture::new();
        let catalog = fixture
            .repository
            .load_catalog()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let configuration = fixture
            .repository
            .load_configuration()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let connection = catalog
            .connections
            .get(&fixture.connection)
            .unwrap_or_else(|| panic!("旧 oracle 连接"));
        let profile = configuration
            .profiles
            .iter()
            .find(|profile| profile.id == fixture.profile)
            .unwrap_or_else(|| panic!("旧 oracle profile"));
        let expected_resolved = profile
            .settings
            .resolve(&connection.terminal_overrides)
            .with_app_key_bindings(&configuration.settings.key_bindings);
        let initial = ROOT.snapshot();
        assert_eq!(initial.rust_bytes, 0);
        let scope = audit::AuditScope::start(&ROOT);
        let record = OwnedRecord::load(fixture.inputs(), &ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let provider = record
            .try_clone()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let persistent_record = record.admitted().rust_bytes;
        let mut pool = HelperPool::new(&ROOT, 1).unwrap_or_else(|error| panic!("{error:?}"));
        let persistent = ROOT.snapshot().rust_bytes;
        let started = Instant::now();
        let (ticket, charge) = submit(&pool, &fixture, &record, false);
        let completion = wait(&pool, ticket);
        assert_eq!(completion.exit_code(), 0);
        assert_eq!(completion.stderr_bytes(), 0);
        assert_eq!(completion.helper_admitted().rust_bytes, charge.helper);
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Helper as usize].c_bytes,
            charge.c
        );
        let wire_bytes = completion.raw().len();
        let loaded = completion
            .into_loaded()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let facts = loaded.required_facts();
        assert_eq!(facts.group_reference(), GroupReferenceFact::NoReference);
        assert_eq!(
            facts.selected_connection(),
            Some(*fixture.connection.0.as_bytes())
        );
        assert_eq!(facts.transaction_nonce(), [7; 16]);
        assert_eq!(facts.source(), 2);
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Helper as usize].rust_bytes,
            0
        );
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Raw as usize].rust_bytes,
            charge.raw
        );
        let target = certificate::TargetLayouts::foundation(certificate::GNU_1981)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let mut calls = ExecutionCounts::default();
        let build = helper_artifact::decode_loaded(
            &loaded,
            fixture.selection(false),
            &target,
            &ROOT,
            ADMISSION,
            &mut calls,
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(&build.snapshot.value().target, connection);
        assert_eq!(&build.snapshot.value().profile, profile);
        assert_eq!(build.snapshot.value().app, configuration.settings);
        assert_eq!(build.snapshot.value().resolved, expected_resolved);
        assert_eq!(build.snapshot.value().revision, facts.revision());
        assert_eq!(build.snapshot.value().source as u32, facts.source());
        let second = build
            .snapshot
            .try_clone()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let retained = build.snapshot.admitted().rust_bytes;
        drop(build);
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Decode as usize].rust_bytes,
            retained
        );
        drop(loaded);
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Raw as usize].rust_bytes,
            0
        );
        pool.shutdown().unwrap_or_else(|error| panic!("{error:?}"));
        drop(record);
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Persistent as usize].rust_bytes,
            persistent_record
        );
        drop(second);
        drop(provider);
        let report = scope.finish();
        assert!(!report.overflow);
        let mut live = 0usize;
        let mut peak = 0usize;
        for event in report.events() {
            let charged_peak = match event.kind {
                1 => {
                    live += event.size;
                    live
                }
                2 => {
                    let before = live;
                    live -= event.size;
                    before
                }
                3 => {
                    let before = live + event.size;
                    live = live - event.old_size + event.size;
                    before
                }
                _ => panic!("正常 audit 不应有 allocation failure"),
            };
            assert!(
                event.held_rust >= charged_peak,
                "每笔旧新共存及 dealloc 前都必须持有足额 grant"
            );
            peak = peak.max(charged_peak);
        }
        assert_eq!(live, 0);
        assert_eq!(ROOT.snapshot(), initial);
        println!(
            "candidate raw={wire_bytes} H={} C={} spawn={} raw_grant={} persistent={} retained={} actual_peak={} snapshot_inline={} owner_inline={} elapsed_ms={} audit_events={} refund=0",
            charge.helper,
            charge.c,
            charge.spawn,
            charge.raw,
            persistent,
            retained,
            peak,
            domain_decode::SnapshotBody::actual_layout().size(),
            crate::owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
                .unwrap_or_else(|error| panic!("{error:?}"))
                .backing
                .size(),
            started.elapsed().as_millis(),
            report.events().len()
        );
    }

    #[test]
    fn artifact_required_group_facts_and_codec_order() {
        let fixture = Fixture::new();
        let group = GroupId(Uuid::from_u128(9));
        fixture
            .repository
            .apply(CatalogMutation::CreateGroup(ConnectionGroup {
                id: group,
                parent_id: None,
                name: "合成组".into(),
                position: 0,
            }))
            .unwrap_or_else(|error| panic!("{error:?}"));
        let record = OwnedRecord::load(fixture.inputs(), &ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let mut pool = HelperPool::new(&ROOT, 1).unwrap_or_else(|error| panic!("{error:?}"));
        let target = certificate::TargetLayouts::foundation(certificate::GNU_1981)
            .unwrap_or_else(|error| panic!("{error:?}"));
        for (text, fact) in [
            (
                "{00000000-0000-0000-0000-000000000009}",
                GroupReferenceFact::Exists,
            ),
            (
                "00000000000000000000000000000008",
                GroupReferenceFact::Absent,
            ),
            ("not-uuid", GroupReferenceFact::InvalidUuid),
        ] {
            fixture.update(c"UPDATE connections SET group_id=?1", text);
            let (ticket, _) = submit(&pool, &fixture, &record, false);
            let loaded = wait(&pool, ticket)
                .into_loaded()
                .unwrap_or_else(|error| panic!("{error:?}"));
            assert_eq!(loaded.required_facts().group_reference(), fact);
            let result = helper_artifact::decode_loaded(
                &loaded,
                fixture.selection(false),
                &target,
                &ROOT,
                ADMISSION,
                &mut ExecutionCounts::default(),
            );
            match fact {
                GroupReferenceFact::Exists => assert_eq!(
                    result
                        .unwrap_or_else(|error| panic!("{error:?}"))
                        .snapshot
                        .value()
                        .target
                        .group_id,
                    Some(group)
                ),
                GroupReferenceFact::Absent => assert!(matches!(
                    result,
                    Err(domain_decode::Failure::MissingGroup(_))
                )),
                _ => assert!(matches!(
                    result,
                    Err(domain_decode::Failure::Corrupt { tag: 2, .. })
                )),
            }
        }
        fixture.update(
            c"UPDATE connections SET group_id=?1",
            "00000000000000000000000000000008",
        );
        fixture.update(c"UPDATE connections SET terminal_overrides_json=?1", "{");
        let (ticket, _) = submit(&pool, &fixture, &record, false);
        let loaded = wait(&pool, ticket)
            .into_loaded()
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert!(matches!(
            helper_artifact::decode_loaded(
                &loaded,
                fixture.selection(false),
                &target,
                &ROOT,
                ADMISSION,
                &mut ExecutionCounts::default()
            ),
            Err(domain_decode::Failure::Corrupt { tag: 16, .. })
        ));
        drop(loaded);
        fixture.update(c"UPDATE connections SET group_id=?1", "not-uuid");
        fixture.update(c"UPDATE connections SET id=?1", "invalid-connection");
        // 请求选中原 binary UUID，缺失连接是 helper 结果，不伪造 LoadedRaw。
        let (ticket, _) = submit(&pool, &fixture, &record, false);
        assert!(matches!(
            wait(&pool, ticket).into_loaded(),
            Err(Failure::Helper)
        ));
        pool.shutdown().unwrap_or_else(|error| panic!("{error:?}"));
        drop(record);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
    }

    #[test]
    fn artifact_normal_quick_candidate_defaults_without_catalog_load() {
        use domain_decode::raw::QuickTarget;
        let fixture = Fixture::new();
        let record = OwnedRecord::load(fixture.inputs(), &ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let args = [record.database()];
        let identity = LaunchIdentity {
            executable: record.executable(),
            arguments: &args,
        };
        let shape = helper_artifact::model::launch_shape(identity)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let h = record
            .helper_costs(identity, &shape)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .total()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let mut bytes = request(&fixture, h, false).bytes;
        bytes[10..12].fill(0);
        bytes[40..72].fill(0);
        bytes[72..80].copy_from_slice(&(64u64 + 24 * 6 + 1048576 + 128).to_le_bytes());
        bytes[88..92].copy_from_slice(&6u32.to_le_bytes());
        let request = Request::parse(bytes, helper_artifact::REQUEST_MAGIC)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let launch = unsafe { TrustedLaunch::borrowed(record.executable(), &args) }
            .unwrap_or_else(|error| panic!("{error:?}"));
        let mut pool = HelperPool::new(&ROOT, 1).unwrap_or_else(|error| panic!("{error:?}"));
        let plan = SpawnCostPlan::bound(&launch, &request, &record)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let reservations = plan
            .costs()
            .reserve(&ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let ticket = pool
            .submit_reserved(
                Submission {
                    launch: &launch,
                    bytes,
                    record: &record,
                    admission: ADMISSION,
                    timeout_ms: 10000,
                },
                reservations,
            )
            .unwrap_or_else(|error| panic!("{error:?}"));
        let loaded = wait(&pool, ticket)
            .into_loaded()
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(loaded.required_facts().selected_connection(), None);
        assert_eq!(
            loaded.required_facts().group_reference(),
            GroupReferenceFact::NoReference
        );
        assert_eq!(loaded.required_facts().source(), 3);
        let configuration = fixture
            .repository
            .load_configuration()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let profile = configuration
            .profiles
            .iter()
            .find(|profile| profile.id == fixture.profile)
            .unwrap_or_else(|| panic!("旧 default oracle"));
        let quick = QuickTarget {
            id: ConnectionId(Uuid::from_u128(10)),
            host: " fixture.invalid ",
            port: 22,
            username: " fixture ",
            command: " printf fixture ",
        };
        let selection = Selection {
            connection: None,
            explicit_profile: None,
            expected_transport: TransportKind::NativeSsh,
            quick: Some(quick),
        };
        let target = certificate::TargetLayouts::foundation(certificate::GNU_1981)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let build = helper_artifact::decode_loaded(
            &loaded,
            selection,
            &target,
            &ROOT,
            ADMISSION,
            &mut ExecutionCounts::default(),
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        assert!(!build.snapshot.value().saved);
        assert_eq!(&build.snapshot.value().profile, profile);
        assert_eq!(build.snapshot.value().app, configuration.settings);
        assert_eq!(build.snapshot.value().target.host, "fixture.invalid");
        assert_eq!(build.snapshot.value().target.username, "fixture");
        assert_eq!(
            build.snapshot.value().target.remote_command.as_deref(),
            Some("printf fixture")
        );
        drop(build);
        drop(loaded);
        pool.shutdown().unwrap_or_else(|error| panic!("{error:?}"));
        drop(record);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
    }

    #[test]
    fn artifact_h_before_allocation_and_late_cancel_no_eof_refund() {
        let fixture = Fixture::new();
        let record = OwnedRecord::load(fixture.inputs(), &ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let args = [record.database()];
        let launch = unsafe { TrustedLaunch::borrowed(record.executable(), &args) }
            .unwrap_or_else(|error| panic!("{error:?}"));
        let scope = audit::AuditScope::start(&ROOT);
        assert!(matches!(
            SpawnCostPlan::bound(&launch, &request(&fixture, 1, false), &record),
            Err(Failure::Budget)
        ));
        let mut zero = request(&fixture, 1, false).bytes;
        zero[80..88].fill(0);
        assert!(matches!(
            Request::parse(zero, helper_artifact::REQUEST_MAGIC),
            Err(Failure::Protocol)
        ));
        assert!(scope.finish().events().is_empty());
        let identity = LaunchIdentity {
            executable: record.executable(),
            arguments: &args,
        };
        let shape = helper_artifact::model::launch_shape(identity)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let h = record
            .helper_costs(identity, &shape)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .total()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let request = request(&fixture, h, false);
        let scope = audit::AuditScope::start(&ROOT);
        let authorized = record
            .check(identity, &shape, &request)
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(authorized.amount.rust_bytes, h);
        let mut console = shape;
        console.inherits_environment = false;
        assert!(matches!(
            record.check(identity, &console, &request),
            Err(Reject::Launch)
        ));
        let extra = [record.database(), OsStr::new("extra")];
        assert!(matches!(
            record.check(
                LaunchIdentity {
                    executable: record.executable(),
                    arguments: &extra
                },
                &shape,
                &request
            ),
            Err(Reject::Launch)
        ));
        assert!(scope.finish().events().is_empty());
        let mut pool = HelperPool::new(&ROOT, 1).unwrap_or_else(|error| panic!("{error:?}"));
        let (ticket, charge) = submit(&pool, &fixture, &record, false);
        let deadline = Instant::now() + Duration::from_secs(20);
        while pool.snapshot()[ticket.slot] & 8 == 0 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Helper as usize].rust_bytes,
            charge.helper
        );
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Raw as usize].rust_bytes,
            charge.raw
        );
        pool.cancel(ticket)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let completion = wait(&pool, ticket);
        assert!(matches!(completion.into_loaded(), Err(Failure::Cancelled)));
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Helper as usize].rust_bytes,
            0
        );
        assert_eq!(
            ROOT.snapshot().classes[ResourceClass::Raw as usize].rust_bytes,
            0
        );
        pool.shutdown().unwrap_or_else(|error| panic!("{error:?}"));
        drop(record);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
    }

    #[test]
    fn artifact_bound_record_mutation_is_rejected_and_refunded() {
        let fixture = Fixture::new();
        let copy = fixture.directory.join("record.json");
        let original = std::fs::read(&fixture.record).unwrap_or_else(|_| panic!("真实记录"));
        for (needle, replacement, expected) in [
            ("\"1.99.0\"", "\"1.98.1\"", Reject::Compiler),
            (
                "\"RUSTFLAGS\": null",
                "\"RUSTFLAGS\": \"altered\"",
                Reject::Cfg,
            ),
            ("\"test\": false", "\"test\": true", Reject::Cfg),
            (
                helper_artifact::ARTIFACT_SHA256,
                "a7542ca44ac1504c4a9c6fc25ce77680a52ebf9eedd559b2c7deef8dabc2a774",
                Reject::Artifact,
            ),
        ] {
            let text = std::str::from_utf8(&original)
                .unwrap_or_else(|_| panic!("record UTF8"))
                .replacen(needle, replacement, 1);
            std::fs::write(&copy, text).unwrap_or_else(|_| panic!("本轮 record copy"));
            let scope = audit::AuditScope::start(&ROOT);
            let result = OwnedRecord::load(
                Inputs {
                    record: &copy,
                    ..fixture.inputs()
                },
                &ROOT,
                ADMISSION,
            );
            assert!(matches!(result, Err(error) if error == expected));
            let report = scope.finish();
            assert!(!report.overflow);
            assert!(report.symmetric_single_backing());
            assert_eq!(ROOT.snapshot().rust_bytes, 0);
        }
        let tiny = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
        let scope = audit::AuditScope::start(&tiny);
        assert!(matches!(
            OwnedRecord::load(fixture.inputs(), &tiny, ADMISSION),
            Err(Reject::Budget)
        ));
        assert!(scope.finish().events().is_empty());
        let record = OwnedRecord::load(fixture.inputs(), &ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let wrong = [OsStr::new("different")];
        let identity = LaunchIdentity {
            executable: record.executable(),
            arguments: &wrong,
        };
        let shape = helper_artifact::model::launch_shape(identity)
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert!(matches!(
            record.helper_costs(identity, &shape),
            Err(Reject::Launch)
        ));
        drop(record);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
    }

    fn record_with_extra(original: &str, extra: &str) -> String {
        let prefix = original
            .trim_end()
            .strip_suffix('}')
            .unwrap_or_else(|| panic!("record object"));
        format!("{prefix},\"extra\":{extra}}}")
    }

    fn rejects_record_escapes(cases: &[&str]) {
        let fixture = Fixture::new();
        let directory = fixture.directory.as_path().to_path_buf();
        let copy = fixture.directory.join("unicode-record.json");
        let original =
            std::fs::read_to_string(&fixture.record).unwrap_or_else(|_| panic!("真实记录"));
        let before = ROOT.snapshot();
        for extra in cases {
            std::fs::write(&copy, record_with_extra(&original, extra))
                .unwrap_or_else(|_| panic!("本轮 record copy"));
            let scope = audit::AuditScope::start(&ROOT);
            let result = OwnedRecord::load(
                Inputs {
                    record: &copy,
                    ..fixture.inputs()
                },
                &ROOT,
                ADMISSION,
            );
            let report = scope.finish();
            assert!(matches!(result, Err(Reject::Record)));
            assert!(!report.overflow);
            assert!(report.symmetric_single_backing());
            for event in report.events() {
                assert_eq!(event.held_rust, before.rust_bytes + event.size);
            }
            assert_eq!(ROOT.snapshot(), before);
        }
        drop(fixture);
        assert!(!directory.exists());
    }

    #[test]
    fn artifact_record_rejects_non_ascii_escape_two_byte_scalar() {
        rejects_record_escapes(&[r#""\é中🦀""#, r#"{"nested":["中🦀\é"]}"#]);
    }

    #[test]
    fn artifact_record_rejects_non_ascii_escape_three_byte_scalar() {
        rejects_record_escapes(&[r#""\中🦀é""#, r#"{"nested":["🦀é\中"]}"#]);
    }

    #[test]
    fn artifact_record_rejects_non_ascii_escape_four_byte_scalar() {
        rejects_record_escapes(&[r#""\🦀é中""#, r#"{"nested":["é中\🦀"]}"#]);
    }

    #[test]
    fn artifact_record_accepts_legal_unicode_and_json_escapes() {
        let fixture = Fixture::new();
        let directory = fixture.directory.as_path().to_path_buf();
        let copy = fixture.directory.join("unicode-record.json");
        let original =
            std::fs::read_to_string(&fixture.record).unwrap_or_else(|_| panic!("真实记录"));
        let before = ROOT.snapshot();
        for extra in [
            r#""é中🦀""#,
            r#""中🦀é""#,
            r#""🦀é中""#,
            r#""\u00e9\u4e2d\ud83e\udd80""#,
            r#"{"nested":["\b\f\n\r\t\"\\\/é中\uD83E\uDD80"]}"#,
        ] {
            std::fs::write(&copy, record_with_extra(&original, extra))
                .unwrap_or_else(|_| panic!("本轮 record copy"));
            let scope = audit::AuditScope::start(&ROOT);
            let record = OwnedRecord::load(
                Inputs {
                    record: &copy,
                    ..fixture.inputs()
                },
                &ROOT,
                ADMISSION,
            )
            .unwrap_or_else(|error| panic!("{error:?}"));
            let charge = record.admitted().rust_bytes;
            drop(record);
            let report = scope.finish();
            assert!(!report.overflow);
            assert!(report.symmetric_single_backing());
            for event in report.events() {
                assert_eq!(event.size, charge);
                assert_eq!(event.held_rust, before.rust_bytes + charge);
            }
            assert_eq!(ROOT.snapshot(), before);
        }
        drop(fixture);
        assert!(!directory.exists());
    }

    #[test]
    fn artifact_record_admission_errors_preserve_source_and_epoch_without_copy() {
        use crate::{
            layout::NumericReject,
            ledger::{SOURCE_COUNT, SourceToken},
        };
        let fixture = Fixture::new();
        let directory = fixture.directory.as_path().to_path_buf();
        assert_eq!(
            Reject::from(NumericReject::RustBudgetExceeded),
            Reject::Budget
        );
        assert_eq!(
            Reject::from(NumericReject::AllocationFailure),
            Reject::Allocation
        );
        assert_eq!(Reject::from(NumericReject::Overflow), Reject::Overflow);
        for case in 0..4 {
            let root = ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT]);
            let source = if case == 1 {
                Some(
                    root.open_source(1, 0)
                        .unwrap_or_else(|error| panic!("{error:?}")),
                )
            } else {
                None
            };
            let canary = OwnedRecord::load(fixture.inputs(), &root, Admission { epoch: 1, source })
                .unwrap_or_else(|error| panic!("{error:?}"));
            let (admission, expected) = match case {
                0 => {
                    root.advance_epoch(1)
                        .unwrap_or_else(|error| panic!("{error:?}"));
                    (ADMISSION, NumericReject::EpochMismatch)
                }
                1 => {
                    root.invalidate_source(source.unwrap_or_else(|| panic!("source token")))
                        .unwrap_or_else(|error| panic!("{error:?}"));
                    (
                        Admission { epoch: 1, source },
                        NumericReject::SourceMismatch,
                    )
                }
                2 => (
                    Admission {
                        epoch: 1,
                        source: Some(SourceToken {
                            epoch: 1,
                            generation: 0,
                            index: SOURCE_COUNT as u32,
                        }),
                    },
                    NumericReject::InvalidSource,
                ),
                _ => (
                    Admission {
                        epoch: 1,
                        source: Some(SourceToken {
                            epoch: 1,
                            generation: 0,
                            index: 0,
                        }),
                    },
                    NumericReject::SourceMismatch,
                ),
            };
            let before = root.snapshot();
            let scope = audit::AuditScope::start(&root);
            let error = OwnedRecord::load(fixture.inputs(), &root, admission)
                .err()
                .unwrap_or_else(|| panic!("失效准入不能复制 metadata"));
            let report = scope.finish();
            assert!(report.events().is_empty());
            assert_eq!(root.snapshot(), before);
            assert_eq!(root.snapshot().rust_bytes, canary.admitted().rust_bytes);
            assert_ne!(error, Reject::Overflow);
            assert_eq!(error, Reject::Admission(expected));
            drop(canary);
            assert_eq!(root.snapshot().rust_bytes, 0);
        }
        assert_ne!(
            Reject::from(NumericReject::EpochMismatch),
            Reject::from(NumericReject::SourceMismatch)
        );
        assert_ne!(
            Reject::from(NumericReject::SourceMismatch),
            Reject::from(NumericReject::InvalidSource)
        );
        let zero = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
        zero.advance_epoch(1)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let scope = audit::AuditScope::start(&zero);
        let error = OwnedRecord::load(fixture.inputs(), &zero, ADMISSION)
            .err()
            .unwrap_or_else(|| panic!("旧 epoch 准入"));
        let report = scope.finish();
        assert_ne!(error, Reject::Budget);
        assert_eq!(error, Reject::Admission(NumericReject::EpochMismatch));
        assert!(report.events().is_empty());
        assert_eq!(zero.snapshot().rust_bytes, 0);
        drop(fixture);
        assert!(!directory.exists());
    }

    #[test]
    fn artifact_profiles_aliases_and_large_unrelated_graph() {
        let fixture = Fixture::new();
        let record = OwnedRecord::load(fixture.inputs(), &ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let mut pool = HelperPool::new(&ROOT, 1).unwrap_or_else(|error| panic!("{error:?}"));
        let target = certificate::TargetLayouts::foundation(certificate::GNU_1981)
            .unwrap_or_else(|error| panic!("{error:?}"));
        for alias in [
            "00000000000000000000000000000003",
            "{00000000-0000-0000-0000-000000000003}",
            "urn:uuid:00000000-0000-0000-0000-000000000003",
        ] {
            fixture.update(c"UPDATE connections SET id=?1", alias);
            fixture.update(c"UPDATE connection_tags SET connection_id=?1", alias);
            let catalog = fixture
                .repository
                .load_catalog()
                .unwrap_or_else(|error| panic!("{error:?}"));
            let (ticket, _) = submit(&pool, &fixture, &record, true);
            let loaded = wait(&pool, ticket)
                .into_loaded()
                .unwrap_or_else(|error| panic!("{error:?}"));
            assert_eq!(loaded.required_facts().source(), 1);
            let build = helper_artifact::decode_loaded(
                &loaded,
                fixture.selection(true),
                &target,
                &ROOT,
                ADMISSION,
                &mut ExecutionCounts::default(),
            )
            .unwrap_or_else(|error| panic!("{error:?}"));
            assert_eq!(
                &build.snapshot.value().target,
                catalog
                    .connections
                    .get(&fixture.connection)
                    .unwrap_or_else(|| panic!("旧 UUID oracle"))
            );
        }
        fixture.update(
            c"UPDATE connections SET terminal_profile_id=NULL WHERE ?1 IS NOT NULL",
            "fixture",
        );
        let (ticket, _) = submit(&pool, &fixture, &record, false);
        let loaded = wait(&pool, ticket)
            .into_loaded()
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(loaded.required_facts().source(), 3);
        helper_artifact::decode_loaded(
            &loaded,
            fixture.selection(false),
            &target,
            &ROOT,
            ADMISSION,
            &mut ExecutionCounts::default(),
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        drop(loaded);
        let unrelated_id = TerminalProfileId(Uuid::from_u128(4));
        let mut unrelated = TerminalSettingsV1::default();
        unrelated.extensions.insert(
            "unrelated".into(),
            serde_json::Value::String("x".repeat(2 * 1024 * 1024)),
        );
        fixture
            .repository
            .save_terminal_profile(TerminalProfile {
                id: unrelated_id,
                name: "不相关".into(),
                settings: unrelated,
            })
            .unwrap_or_else(|error| panic!("{error:?}"));
        fixture.update(c"UPDATE terminal_profiles SET settings_json=?1 WHERE id='00000000-0000-0000-0000-000000000004'", "{");
        assert!(
            fixture.repository.load_configuration().is_err(),
            "旧 bulk oracle 确实触及坏的无关字段"
        );
        let (ticket, _) = submit(&pool, &fixture, &record, false);
        let loaded = wait(&pool, ticket)
            .into_loaded()
            .unwrap_or_else(|error| panic!("{error:?}"));
        helper_artifact::decode_loaded(
            &loaded,
            fixture.selection(false),
            &target,
            &ROOT,
            ADMISSION,
            &mut ExecutionCounts::default(),
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        drop(loaded);
        fixture.update(
            c"UPDATE terminal_profiles SET id=?1 WHERE id='00000000-0000-0000-0000-000000000002'",
            "00000000-0000-0000-0000-000000000005",
        );
        let (ticket, _) = submit(&pool, &fixture, &record, true);
        let completion = wait(&pool, ticket);
        assert_eq!(
            completion
                .result()
                .unwrap_or_else(|error| panic!("{error:?}"))
                .outcome,
            2
        );
        assert!(matches!(completion.into_loaded(), Err(Failure::Helper)));
        pool.shutdown().unwrap_or_else(|error| panic!("{error:?}"));
        drop(record);
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
    }

    #[test]
    fn artifact_actual_source_and_image_copy_mutation_rejected() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let fixture = Fixture::new();
        let tree = fixture.directory.join("sources");
        assert!(fixture.directory.is_dir());
        std::fs::create_dir(&tree).unwrap_or_else(|_| panic!("本轮源校验树"));
        let recipe: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&fixture.recipe).unwrap_or_else(|_| panic!("recipe")),
        )
        .unwrap_or_else(|_| panic!("recipe JSON"));
        for entry in recipe["sources"]["files"]
            .as_array()
            .unwrap_or_else(|| panic!("源清单"))
        {
            let relative = entry["relativePath"]
                .as_str()
                .unwrap_or_else(|| panic!("相对源路径"));
            let destination = tree.join(relative);
            let parent = destination
                .parent()
                .unwrap_or_else(|| panic!("复制目标 parent"));
            std::fs::create_dir_all(parent).unwrap_or_else(|_| panic!("本轮源 parent"));
            std::fs::copy(fixture.sources.join(relative), &destination)
                .unwrap_or_else(|_| panic!("受控源副本"));
        }
        let copied = OwnedRecord::load(
            Inputs {
                source_root: &tree,
                ..fixture.inputs()
            },
            &ROOT,
            ADMISSION,
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        drop(copied);
        let source = tree.join("crates/rshell-storage/src/bin/selected_read_helper/normal_v2.rs");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source)
            .unwrap_or_else(|_| panic!("本轮源 mutation"));
        let mut byte = [0];
        file.read_exact(&mut byte)
            .unwrap_or_else(|_| panic!("源 byte"));
        file.seek(SeekFrom::Start(0))
            .unwrap_or_else(|_| panic!("源 seek"));
        byte[0] ^= 1;
        file.write_all(&byte)
            .unwrap_or_else(|_| panic!("源 byte write"));
        drop(file);
        assert!(matches!(
            OwnedRecord::load(
                Inputs {
                    source_root: &tree,
                    ..fixture.inputs()
                },
                &ROOT,
                ADMISSION
            ),
            Err(Reject::Source)
        ));
        let executable = fixture.directory.join("copied-helper.exe");
        std::fs::copy(&fixture.candidate, &executable).unwrap_or_else(|_| panic!("实际映像副本"));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&executable)
            .unwrap_or_else(|_| panic!("映像副本 mutation"));
        file.seek(SeekFrom::Start(512))
            .unwrap_or_else(|_| panic!("映像 seek"));
        file.read_exact(&mut byte)
            .unwrap_or_else(|_| panic!("映像 byte"));
        file.seek(SeekFrom::Start(512))
            .unwrap_or_else(|_| panic!("映像 seek"));
        byte[0] ^= 1;
        file.write_all(&byte)
            .unwrap_or_else(|_| panic!("映像 byte write"));
        drop(file);
        let original =
            std::fs::read_to_string(&fixture.record).unwrap_or_else(|_| panic!("record"));
        let old = serde_json::to_string(
            fixture
                .candidate
                .to_str()
                .unwrap_or_else(|| panic!("原映像 UTF8")),
        )
        .unwrap_or_else(|_| panic!("old JSON"));
        let new = serde_json::to_string(
            executable
                .to_str()
                .unwrap_or_else(|| panic!("本轮映像 UTF8")),
        )
        .unwrap_or_else(|_| panic!("new JSON"));
        let record = fixture.directory.join("copied-artifact-record.json");
        std::fs::write(&record, original.replacen(&old, &new, 1))
            .unwrap_or_else(|_| panic!("副本记录"));
        assert!(matches!(
            OwnedRecord::load(
                Inputs {
                    record: &record,
                    executable: executable.as_os_str(),
                    ..fixture.inputs()
                },
                &ROOT,
                ADMISSION
            ),
            Err(Reject::Artifact)
        ));
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
    }

    #[test]
    fn artifact_pool_isolates_real_eof_and_stderr_faults() {
        use crate::process::{CertifiedHelper, HelperBuildRecord, LaunchShape};
        use sha2::{Digest, Sha256};
        use std::os::windows::fs::OpenOptionsExt;

        // 本测试专用无 std/custom entry 闭包：kernel32 同步 IO、栈 request 与静态
        // 合成 wire，无 Rust runtime/args/stdio/allocator。不是正常 helper 构建证书。
        // 两种映像分别必定截断 footer 或输出 stderr，均不能签发 LoadedRaw。
        struct FaultBuild<'a> {
            executable: &'a OsStr,
            database: &'a OsStr,
            request: Request,
            sha256: [u8; 32],
            _immutable: std::fs::File,
        }
        // SAFETY: 仅下方 exact GNU 编译生成的 no_std custom-entry 测试映像，已校验
        // source/flags/实际 SHA 并持排他映像 handle。完整 exe/DB/SHSELREQ 与启动
        // shape 必须匹配；源闭包没有 Rust allocation，正 request H 仍完整占账。
        unsafe impl HelperBuildRecord for FaultBuild<'_> {
            fn request_magic(&self) -> &[u8; 8] {
                helper_artifact::REQUEST_MAGIC
            }
            fn authorize(
                &self,
                identity: LaunchIdentity<'_>,
                shape: &LaunchShape,
                request: &Request,
            ) -> Result<CertifiedHelper, Failure> {
                if identity.executable != self.executable
                    || identity.arguments != [self.database]
                    || *request != self.request
                    || !shape.inherits_environment
                    || !shape.inherits_cwd
                {
                    return Err(Failure::Unsupported);
                }
                let actual = helper_artifact::model::launch_shape(identity)
                    .map_err(|_| Failure::Unsupported)?;
                if shape.arguments != actual.arguments
                    || shape.argument_units != actual.argument_units
                    || shape.executable_units != actual.executable_units
                    || shape.command_units != actual.command_units
                {
                    return Err(Failure::Unsupported);
                }
                Ok(CertifiedHelper {
                    amount: crate::ledger::Amount {
                        rust_bytes: request.helper_rust,
                        c_bytes: request.c_max,
                        slots: 1,
                    },
                    artifact_sha256: self.sha256,
                })
            }
        }

        let fixture = Fixture::new();
        let record = OwnedRecord::load(fixture.inputs(), &ROOT, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let mut pool = HelperPool::new(&ROOT, 1).unwrap_or_else(|error| panic!("{error:?}"));
        let (ticket, _) = submit(&pool, &fixture, &record, false);
        let loaded = wait(&pool, ticket)
            .into_loaded()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let wire = fixture.directory.join("wire.bin");
        std::fs::write(&wire, loaded.storage().bytes())
            .unwrap_or_else(|_| panic!("本轮 synthetic wire"));
        let wire_len = loaded.storage().len();
        drop(loaded);
        drop(record);
        let request = request(&fixture, 1, false);
        std::fs::write(fixture.directory.join("request.bin"), request.bytes)
            .unwrap_or_else(|_| panic!("本轮 synthetic request"));
        let persistent = ROOT.snapshot().rust_bytes;
        for stderr in [false, true] {
            let output_len = if stderr { wire_len } else { wire_len - 1 };
            let code = format!(
                r#"#![no_std]
#![no_main]
use core::ffi::c_void;
#[link(name = "kernel32")]
unsafe extern "system" {{
    fn GetStdHandle(kind: u32) -> *mut c_void;
    fn ReadFile(handle: *mut c_void, bytes: *mut u8, len: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn WriteFile(handle: *mut c_void, bytes: *const u8, len: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
}}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {{ unsafe {{ ExitProcess(12) }} }}
// GNU 自带 crt2 的链接符号；PE entry 由 --entry 固定，不执行 CRT main。
#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {{ 0 }}
#[unsafe(no_mangle)]
pub unsafe extern "system" fn selected_fixture_entry() -> ! {{
    let mut request = [0u8; 112];
    let mut at = 0;
    while at < request.len() {{
        let mut count = 0;
        let ok = unsafe {{ ReadFile(GetStdHandle(-10i32 as u32), request.as_mut_ptr().add(at), (request.len() - at) as u32, &mut count, core::ptr::null_mut()) }};
        if ok == 0 || count == 0 {{ unsafe {{ ExitProcess(13) }} }}
        at += count as usize;
    }}
    if &request != include_bytes!("request.bin") {{ unsafe {{ ExitProcess(14) }} }}
    let bytes = include_bytes!("wire.bin");
    let mut at = 0usize;
    while at < {output_len} {{
        let mut count = 0;
        let ok = unsafe {{ WriteFile(GetStdHandle(-11i32 as u32), bytes.as_ptr().add(at), ({output_len} - at) as u32, &mut count, core::ptr::null_mut()) }};
        if ok == 0 || count == 0 {{ unsafe {{ ExitProcess(15) }} }}
        at += count as usize;
    }}
    if {stderr} {{
        let mut count = 0;
        if unsafe {{ WriteFile(GetStdHandle(-12i32 as u32), b"x".as_ptr(), 1, &mut count, core::ptr::null_mut()) }} == 0 || count != 1 {{ unsafe {{ ExitProcess(16) }} }}
    }}
    unsafe {{ ExitProcess(0) }}
}}
"#
            );
            let source = fixture.directory.join("fault.rs");
            let executable = fixture
                .directory
                .join(if stderr { "stderr.exe" } else { "eof.exe" });
            std::fs::write(&source, &code).unwrap_or_else(|_| panic!("本轮无 std fixture"));
            let output = std::process::Command::new("rustup")
                .args([
                    "run",
                    "1.98.1-x86_64-pc-windows-gnu",
                    "rustc",
                    "--edition=2024",
                    "--crate-type=bin",
                    "-C",
                    "panic=abort",
                    "-C",
                    "opt-level=1",
                    "-C",
                    "overflow-checks=yes",
                    "-C",
                    "link-arg=-nostartfiles",
                    "-C",
                    "link-arg=-Wl,--entry,selected_fixture_entry",
                ])
                .arg(&source)
                .arg("-o")
                .arg(&executable)
                .env("RUSTUP_AUTO_INSTALL", "0")
                .output()
                .unwrap_or_else(|_| panic!("现有 exact GNU compiler"));
            assert!(
                output.status.success(),
                "隔离 fixture 编译失败: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                std::fs::read(&source).unwrap_or_else(|_| panic!("source hash")),
                code.as_bytes()
            );
            let sha256: [u8; 32] = Sha256::digest(
                std::fs::read(&executable).unwrap_or_else(|_| panic!("实际 fixture SHA")),
            )
            .into();
            let build = FaultBuild {
                executable: executable.as_os_str(),
                database: fixture.database.as_os_str(),
                request,
                sha256,
                _immutable: std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .open(&executable)
                    .unwrap_or_else(|_| panic!("排他 fixture 映像")),
            };
            let args = [build.database];
            let launch = unsafe { TrustedLaunch::borrowed(build.executable, &args) }
                .unwrap_or_else(|error| panic!("{error:?}"));
            let scope = audit::AuditScope::start(&ROOT);
            let plan = SpawnCostPlan::bound(&launch, &request, &build)
                .unwrap_or_else(|error| panic!("{error:?}"));
            let reservations = plan
                .costs()
                .reserve(&ROOT, ADMISSION)
                .unwrap_or_else(|error| panic!("{error:?}"));
            let ticket = pool
                .submit_reserved(
                    Submission {
                        launch: &launch,
                        bytes: request.bytes,
                        record: &build,
                        admission: ADMISSION,
                        timeout_ms: 10000,
                    },
                    reservations,
                )
                .unwrap_or_else(|error| panic!("{error:?}"));
            let completion = wait(&pool, ticket);
            if stderr {
                assert_eq!(completion.exit_code(), 0);
                assert_eq!(completion.stderr_bytes(), 1);
                assert!(matches!(completion.into_loaded(), Err(Failure::Protocol)));
            } else {
                assert!(matches!(completion.into_loaded(), Err(Failure::Truncated)));
            }
            let report = scope.finish();
            assert!(!report.overflow);
            assert_eq!(ROOT.snapshot().rust_bytes, persistent);
            assert_eq!(
                ROOT.snapshot().classes[ResourceClass::Raw as usize].rust_bytes,
                0
            );
            assert_eq!(
                ROOT.snapshot().classes[ResourceClass::Helper as usize].rust_bytes,
                0
            );
        }
        pool.shutdown().unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(ROOT.snapshot().rust_bytes, 0);
    }
}
