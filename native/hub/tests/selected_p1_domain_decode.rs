//! W1-D 独立 exe：真实依赖 oracle 在审计窗外；测试 allocator 不进入产品。

#[path = "selected_p1_foundation.rs"]
mod foundation;
pub use foundation::{audit, capacity, certificate, layout, ledger, owner, secret};
#[path = "../src/selected_p1/decode_shape/mod.rs"]
pub mod decode_shape;
#[path = "../src/selected_p1/domain_decode/mod.rs"]
pub mod domain_decode;

pub mod selected_p1 {
    pub use crate::decode_shape;
    pub use crate::foundation::selected_p1::{model, resource};
}

use domain_decode::{
    ExecutionCounts, Failure,
    raw::{
        FieldValue, GroupPresence, ProfileSource, RawField, RawFields, RawIdentity,
        SelectedReferenceToken, SelectedReferences, Selection,
    },
};
use ledger::{Admission, BYTE_LIMIT, CLASS_COUNT, ProcessLedger};
use rshell_m0::rshell_core::{
    AppSettings, AuthenticationKind, CatalogMutation, ColorScheme, ConnectionId, ConnectionProfile,
    GroupId, KeyBinding, KeyCode, KeyModifiers, TerminalOverrides, TerminalProfile,
    TerminalProfileId, TerminalSettingsV1, TransportKind,
};
use rshell_m0::rshell_storage::SqliteRepository;
use std::alloc::Layout;
use uuid::Uuid;

#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
use domain_decode::raw::QuickTarget;
use rshell_m0::rshell_core::ConnectionGroup;

const ADMISSION: Admission = Admission {
    epoch: 1,
    source: None,
};

fn target() -> certificate::TargetLayouts {
    certificate::TargetLayouts::foundation(certificate::GNU_1981)
        .unwrap_or_else(|error| panic!("{error:?}"))
}

fn root() -> ProcessLedger {
    ProcessLedger::new(BYTE_LIMIT, [u32::MAX; CLASS_COUNT])
}

fn binding(code: KeyCode, action: &str) -> KeyBinding {
    KeyBinding {
        code,
        modifiers: KeyModifiers::default(),
        action: action.to_owned(),
    }
}

struct Fixture {
    text: [String; 22],
    connection: ConnectionProfile,
    profile: TerminalProfile,
    app: AppSettings,
    tags: Vec<String>,
}

impl Fixture {
    fn ordinary() -> Self {
        let id = TerminalProfileId(Uuid::from_u128(2));
        let settings = TerminalSettingsV1 {
            terminal_type: "  xterm-custom  ".into(),
            font_family: "字体\0族".into(),
            answerback: "回应🦀".into(),
            key_bindings: vec![binding(KeyCode::Enter, "new_tab")],
            extensions: serde_json::from_value(serde_json::json!({
                "vendor.font": {"version": 91, "nested": [true, null, "原始\0🦀", {"x": [1, 2.5]}]},
                "future": {"extra": {"a": "b"}}
            }))
            .unwrap_or_else(|error| panic!("{error}")),
            ..TerminalSettingsV1::default()
        };
        let profile = TerminalProfile {
            id,
            name: "选择的配置".into(),
            settings,
        };
        let app = AppSettings {
            default_terminal_profile: id,
            color_scheme: ColorScheme::Nord,
            key_bindings: vec![
                binding(KeyCode::Enter, "split_vertical"),
                binding(KeyCode::F(2), "new_tab"),
            ],
        };
        let mut connection = ConnectionProfile::new("已保存", "fixture.invalid");
        connection.id = ConnectionId(Uuid::from_u128(3));
        connection.transport = TransportKind::NativeSsh;
        connection.authentication = AuthenticationKind::Password;
        connection.username = "用户".into();
        connection.note = "只留 Rust\0私有数据".into();
        connection.identity_file = Some(std::path::PathBuf::from("目录/🦀\0key"));
        connection.credential_ref = Some("opaque-fixture".into());
        connection.remote_command = Some("printf fixture".into());
        connection.terminal_profile_id = Some(id);
        connection.terminal_overrides = TerminalOverrides {
            initial_cols: Some(88),
            key_bindings: Some(vec![binding(KeyCode::Tab, "new_tab")]),
            ..Default::default()
        };
        connection.tags.insert("a".into());
        connection.tags.insert("中文🦀".into());
        Self::from_values(connection, profile, app)
    }

    fn from_values(
        connection: ConnectionProfile,
        profile: TerminalProfile,
        app: AppSettings,
    ) -> Self {
        let mut overrides = serde_json::to_value(&connection.terminal_overrides)
            .unwrap_or_else(|error| panic!("{error}"));
        overrides
            .as_object_mut()
            .unwrap_or_else(|| panic!("override map"))
            .insert("version".into(), 1.into());
        let text = [
            connection.id.0.to_string(),
            connection
                .group_id
                .map(|id| id.0.to_string())
                .unwrap_or_default(),
            connection.name.clone(),
            connection.host.clone(),
            String::new(),
            connection.username.clone(),
            match connection.transport {
                TransportKind::NativeSsh => "native_ssh",
                TransportKind::SystemOpenSsh => "system_open_ssh",
            }
            .into(),
            match connection.authentication {
                AuthenticationKind::Password => "password",
                AuthenticationKind::PublicKey => "public_key",
                AuthenticationKind::Agent => "agent",
                AuthenticationKind::KeyboardInteractive => "keyboard_interactive",
            }
            .into(),
            connection
                .credential_ref
                .as_ref()
                .map(|value| value.0.clone())
                .unwrap_or_default(),
            connection
                .identity_file
                .as_ref()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "strict".into(),
            connection.remote_command.clone().unwrap_or_default(),
            connection.note.clone(),
            String::new(),
            connection
                .terminal_profile_id
                .map(|id| id.0.to_string())
                .unwrap_or_default(),
            serde_json::to_string(&overrides).unwrap_or_else(|error| panic!("{error}")),
            app.default_terminal_profile.0.to_string(),
            "nord".into(),
            serde_json::to_string(
                &serde_json::json!({"version":1,"key_bindings":app.key_bindings}),
            )
            .unwrap_or_else(|error| panic!("{error}")),
            profile.id.0.to_string(),
            profile.name.clone(),
            serde_json::to_string(&profile.settings).unwrap_or_else(|error| panic!("{error}")),
        ];
        let tags = connection.tags.iter().cloned().collect();
        Self {
            text,
            connection,
            profile,
            app,
            tags,
        }
    }

    fn fields(&self, saved: bool) -> Vec<RawField<'_>> {
        let mut fields = Vec::new();
        for tag in if saved { 1..=22 } else { 17..=22 } {
            let text = &self.text[usize::from(tag - 1)];
            let value = match tag {
                5 => FieldValue::Port(self.connection.port),
                14 => FieldValue::Position(self.connection.position),
                2 if self.connection.group_id.is_none() => FieldValue::Null,
                9 if self.connection.credential_ref.is_none() => FieldValue::Null,
                10 if self.connection.identity_file.is_none() => FieldValue::Null,
                12 if self.connection.remote_command.is_none() => FieldValue::Null,
                15 if self.connection.terminal_profile_id.is_none() => FieldValue::Null,
                _ => FieldValue::Text(text),
            };
            fields.push(RawField {
                tag,
                ordinal: 0,
                value,
            });
        }
        if saved {
            for (index, tag) in self.tags.iter().enumerate() {
                fields.push(RawField {
                    tag: 23,
                    ordinal: (index + 1) as u32,
                    value: FieldValue::Text(tag),
                });
            }
        }
        fields
    }

    fn selection(&self) -> Selection<'static> {
        Selection {
            connection: Some(self.connection.id),
            explicit_profile: None,
            expected_transport: TransportKind::NativeSsh,
            quick: None,
        }
    }
}

fn raw<'a>(fixture: &'a Fixture, fields: &'a [RawField<'a>]) -> RawFields<'a> {
    assert!(fixture.connection.group_id.is_none());
    let identity = identity(7);
    let selection = fixture.selection();
    RawFields::checked(
        fields,
        identity,
        ProfileSource::Connection,
        selection,
        no_reference(identity, selection),
    )
    .unwrap_or_else(|error| panic!("{error:?}"))
}

fn identity(revision: u64) -> RawIdentity {
    RawIdentity {
        transaction_nonce: [11; 16],
        revision,
    }
}

fn no_reference(identity: RawIdentity, selection: Selection<'_>) -> SelectedReferences {
    SelectedReferences::from_trusted_transaction(
        SelectedReferenceToken {
            snapshot: identity,
            selected_connection: selection.connection,
        },
        GroupPresence::NoReference,
    )
}

/// 逐笔重放 alloc/realloc/dealloc，realloc 检查旧+新共存而非 delta 或高水位。
/// classes 来自可达源路径：byte/String/scratch/error message、真实 BTree node、typed/Content Vec、owner。
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn verify_audit(report: &audit::Report, snapshot_layout: Layout) {
    assert_eq!(verify_audit_window(report, snapshot_layout, 0), 0);
}

#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn verify_audit_window(report: &audit::Report, snapshot_layout: Layout, mut live: usize) -> usize {
    assert!(!report.overflow);
    let public = layout::PublicLayouts::actual();
    for event in report.events() {
        let class = if event.align == 1 {
            1
        } else if event.size == snapshot_layout.size() && event.align == snapshot_layout.align() {
            2
        } else if event.size == 40 && event.align == 8 {
            3
        } else if matches!(event.size, 280 | 376 | 632 | 728) && event.align == 8 {
            4
        } else if event.align == 8
            && (event.size.is_multiple_of(32)
                || event.size.is_multiple_of(64)
                || event.size.is_multiple_of(public.key_binding.size()))
        {
            5
        } else {
            0
        };
        assert_ne!(class, 0, "未归类 layout {}/{}", event.size, event.align);
        match event.kind {
            1 => {
                live += event.size;
                assert!(live <= event.held_rust, "alloc 未持足额 grant");
            }
            2 => {
                assert!(live <= event.held_rust, "free 前提前返额");
                live -= event.size;
            }
            3 => {
                assert!(live + event.size <= event.held_rust, "realloc 新旧共存超额");
                live = live + event.size - event.old_size;
            }
            4 => {}
            _ => panic!("未知事件"),
        }
    }
    live
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn complete_snapshot_matches_real_storage_and_core_and_last_owner() {
    let fixture = Fixture::ordinary();
    let repository = SqliteRepository::open_in_memory().unwrap_or_else(|error| panic!("{error:?}"));
    repository
        .migrate()
        .unwrap_or_else(|error| panic!("{error:?}"));
    repository
        .save_terminal_profile(fixture.profile.clone())
        .unwrap_or_else(|error| panic!("{error:?}"));
    repository
        .save_settings(fixture.app.clone())
        .unwrap_or_else(|error| panic!("{error:?}"));
    repository
        .apply(CatalogMutation::Create(fixture.connection.clone()))
        .unwrap_or_else(|error| panic!("{error:?}"));
    let catalog = repository
        .load_catalog()
        .unwrap_or_else(|error| panic!("{error:?}"));
    let configuration = repository
        .load_configuration()
        .unwrap_or_else(|error| panic!("{error:?}"));
    let old_target = catalog
        .connections
        .get(&fixture.connection.id)
        .unwrap_or_else(|| panic!("target"));
    let old_profile = configuration
        .profiles
        .iter()
        .find(|profile| profile.id == fixture.profile.id)
        .unwrap_or_else(|| panic!("profile"));
    // storage normalize 后的真实值也成为 raw 输入；生产不执行此 bulk oracle。
    let fixture = Fixture::from_values(
        old_target.clone(),
        old_profile.clone(),
        configuration.settings.clone(),
    );
    repository
        .shutdown()
        .unwrap_or_else(|error| panic!("{error:?}"));
    let fields = fixture.fields(true);
    let raw = raw(&fixture, &fields);
    let target = target();
    let root = root();
    let mut calls = ExecutionCounts::default();
    let audit = audit::AuditScope::start(&root);
    let build = domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(calls, ExecutionCounts { schema: 3, core: 1 });
    assert!(build.plan.rust_bytes < BYTE_LIMIT);
    let plan = build.plan;
    let clone_shape = build.clone_shape;
    let value = build.snapshot.value();
    assert_eq!(value.target, fixture.connection);
    assert_eq!(value.profile, fixture.profile);
    assert_eq!(value.app, fixture.app);
    // oracle clone/resolve 在审计窗外，不能把测试本身的未预约分配算入产品。
    let copy = domain_decode::clone_snapshot(value, &target, &root, ADMISSION)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(copy.value(), value);
    let holder = build
        .snapshot
        .try_clone()
        .unwrap_or_else(|error| panic!("{error:?}"));
    drop(build);
    assert!(root.snapshot().rust_bytes > 0);
    drop(copy);
    drop(holder);
    let report = audit.finish();
    verify_audit(
        &report,
        owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing,
    );
    assert_eq!(root.snapshot().rust_bytes, 0);
    println!(
        "SnapshotBody={}/{} RawField={}/{} Owner={}/{} SmallUpper={} RetainedGraph={} CloneGraph={} Classes={:?}",
        domain_decode::SnapshotBody::actual_layout().size(),
        domain_decode::SnapshotBody::actual_layout().align(),
        Layout::new::<RawField<'_>>().size(),
        Layout::new::<RawField<'_>>().align(),
        owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing
            .size(),
        owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing
            .align(),
        plan.rust_bytes,
        clone_shape.retained_bytes,
        clone_shape.clone_bytes,
        plan.classes
    );
    let old_resolved = fixture
        .profile
        .settings
        .resolve(&fixture.connection.terminal_overrides)
        .with_app_key_bindings(&fixture.app.key_bindings);
    let mut calls = ExecutionCounts::default();
    let build = domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(build.snapshot.value().resolved, old_resolved);
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn shortage_has_no_schema_or_core_execution_and_no_ungranted_allocation() {
    let fixture = Fixture::ordinary();
    let fields = fixture.fields(true);
    let raw = raw(&fixture, &fields);
    let target = target();
    let requirement = domain_decode::budget::scratch_requirement(&raw)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let mut scratch = vec![decode_shape::ScratchFrame::default(); requirement.slots];
    let plan = domain_decode::bound_selected(&raw, &mut scratch, &target)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let root = ProcessLedger::new(
        plan.rust_bytes + requirement.bytes - 1,
        [u32::MAX; CLASS_COUNT],
    );
    let mut calls = ExecutionCounts::default();
    let audit = audit::AuditScope::start(&root);
    let result = domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls);
    assert!(matches!(
        result,
        Err(Failure::Resource(layout::NumericReject::RustBudgetExceeded))
    ));
    assert_eq!(calls, ExecutionCounts::default());
    let report = audit.finish();
    verify_audit(
        &report,
        owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing,
    );
    assert_eq!(root.snapshot().rust_bytes, 0);
    let root = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
    let audit = audit::AuditScope::start(&root);
    assert!(matches!(
        domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls),
        Err(Failure::Resource(layout::NumericReject::RustBudgetExceeded))
    ));
    assert!(audit.finish().events().is_empty());
}

#[test]
fn uuid_all_old_forms_and_invalid_inputs_are_heap_free() {
    let root = root();
    let audit = audit::AuditScope::start(&root);
    for text in [
        "00000000-0000-0000-0000-000000000002",
        "00000000000000000000000000000002",
        "{00000000-0000-0000-0000-000000000002}",
        "urn:uuid:00000000-0000-0000-0000-000000000002",
        "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA",
    ] {
        let id = domain_decode::codec::uuid(text, 20).unwrap_or_else(|error| panic!("{error:?}"));
        let mut buffer = [0; 36];
        assert_eq!(
            Uuid::parse_str(domain_decode::codec::canonical_uuid(id, &mut buffer)),
            Ok(id)
        );
    }
    for text in [
        "",
        "🦀",
        "-",
        "00000000-0000-0000-0000-00000000000g",
        "URN:uuid:00000000-0000-0000-0000-000000000002",
        "00000000-0000-0000-0000-000000000002\0",
    ] {
        assert!(matches!(
            domain_decode::codec::uuid(text, 20),
            Err(Failure::Corrupt {
                tag: 20,
                json: None
            })
        ));
    }
    assert!(audit.finish().events().is_empty());
}

#[test]
fn validators_match_real_core_codes_order_and_do_not_allocate() {
    let mut cases = vec![TerminalSettingsV1::default()];
    for bindings in [
        vec![binding(KeyCode::F(0), "invalid")],
        vec![binding(KeyCode::Character('\0'), "new_tab")],
        vec![binding(KeyCode::F(1), "invalid")],
        vec![
            binding(KeyCode::F(1), "new_tab"),
            binding(KeyCode::F(1), "new_tab"),
        ],
        vec![
            binding(KeyCode::F(1), "new_tab"),
            binding(KeyCode::F(1), "invalid"),
        ],
    ] {
        cases.push(TerminalSettingsV1 {
            key_bindings: bindings,
            ..TerminalSettingsV1::default()
        });
    }
    for field in 0..8 {
        let mut value = TerminalSettingsV1::default();
        match field {
            0 => value.terminal_type.clear(),
            1 => value.initial_cols = 0,
            2 => value.initial_rows = 1000,
            3 => value.scrollback_lines = 99,
            4 => value.font_family.clear(),
            5 => value.font_size = f32::NAN,
            6 => value.font_size = 73.0,
            _ => value.answerback.clear(),
        }
        cases.push(value);
    }
    for value in &cases {
        let expected = rshell_m0::rshell_core::validate_terminal_settings(value);
        let overrides = TerminalOverrides {
            terminal_type: Some(value.terminal_type.clone()),
            initial_cols: Some(value.initial_cols),
            initial_rows: Some(value.initial_rows),
            scrollback_lines: Some(value.scrollback_lines),
            font_family: Some(value.font_family.clone()),
            font_size: Some(value.font_size),
            answerback: Some(value.answerback.clone()),
            key_bindings: Some(value.key_bindings.clone()),
            ..Default::default()
        };
        let expected_override = rshell_m0::rshell_core::validate_terminal_overrides(&overrides);
        let root = root();
        let audit = audit::AuditScope::start(&root);
        assert_eq!(domain_decode::validation::settings(value), expected);
        assert_eq!(
            domain_decode::validation::overrides(&overrides),
            expected_override
        );
        assert!(audit.finish().events().is_empty());
    }
}

#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn decode_case(fixture: &Fixture) -> Result<ExecutionCounts, Failure> {
    let fields = fixture.fields(true);
    let raw = raw(fixture, &fields);
    let root = root();
    let target = target();
    let mut calls = ExecutionCounts::default();
    let audit = audit::AuditScope::start(&root);
    let result =
        domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls).map(drop);
    let report = audit.finish();
    verify_audit(
        &report,
        owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing,
    );
    assert_eq!(root.snapshot().rust_bytes, 0);
    result.map(|()| calls)
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn typed_schema_errors_flatten_duplicates_arrays_and_prefixes_are_admitted_and_audited() {
    for (tag, texts) in [
        (
            16,
            vec![
                "{}",
                "[]",
                "{\"version\":2}",
                "{\"version\":1,\"version\":1}",
                "{\"version\":1,\"font_family\":1}",
                "{\"version\":1,\"font_family\":\"a\",\"font_family\":\"b\"}",
                "{\"version\":1,\"font_family\":\"unterminated",
                "{\"version\":1,\"key_bindings\":[{\"code\":\"bad\"]}",
            ],
        ),
        (
            19,
            vec![
                "{}",
                "[1]",
                "[2,[]]",
                "{\"version\":1,\"key_bindings\":null}",
                "{\"version\":1,\"key_bindings\":[[\"f\",{},\"x\"]]}",
                "{\"version\":1,\"key_bindings\":[{\"code\":{\"character\":\"xx\"},\"modifiers\":{},\"action\":\"x\"}]}",
            ],
        ),
        (
            22,
            vec![
                "{}",
                "[]",
                "{\"version\":2}",
                "{\"version\":1,\"terminal_type\":\"a\",\"terminal_type\":\"b\"}",
                "{\"version\":256}",
                "{\"version\":1,\"terminal_type\":\"\\uD800\"}",
                "{\"version\":1,\"terminal_type\":\"\\u0000🦀\"",
                "{\"version\":1,\"initial_cols\":65536}",
                "{\"version\":1,\"font_size\":1e400}",
            ],
        ),
    ] {
        for text in texts {
            let mut fixture = Fixture::ordinary();
            fixture.text[usize::from(tag - 1)] = text.into();
            let result = decode_case(&fixture);
            assert!(matches!(result, Err(Failure::Corrupt { tag: actual, .. }) if actual == tag));
        }
    }
    let mut fixture = Fixture::ordinary();
    fixture.text[18] = "[1,[]]".into();
    assert!(decode_case(&fixture).is_ok());
    let settings: serde_json::Value =
        serde_json::from_str(&fixture.text[21]).unwrap_or_else(|error| panic!("{error}"));
    let names = [
        "version",
        "terminal_type",
        "initial_cols",
        "initial_rows",
        "scrollback_lines",
        "font_family",
        "font_size",
        "color_scheme",
        "key_bindings",
        "left_alt_as_meta",
        "right_alt_as_meta",
        "enable_csi_u",
        "enable_kitty_keyboard",
        "mouse_reporting",
        "scroll_on_output",
        "scroll_on_keypress",
        "answerback",
        "extensions",
    ];
    fixture.text[21] = serde_json::to_string(&names.map(|name| settings[name].clone()))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(decode_case(&fixture).is_ok());
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn ignored_deep_tree_unpaired_surrogate_and_full_extension_duplicates_keep_acceptance() {
    let mut fixture = Fixture::ordinary();
    let tail = format!(
        ",\"ignored\":{}\"\\uD800\"{}",
        "[".repeat(180),
        "]".repeat(180)
    );
    fixture.text[21].pop();
    fixture.text[21].push_str(&tail);
    fixture.text[21].push('}');
    assert!(decode_case(&fixture).is_ok());
    let mut fixture = Fixture::ordinary();
    fixture.text[15] =
        "{\"version\":1,\"unknown\":{\"x\":\"🦀\\u0000\",\"x\":[1,{\"y\":[true,null]}]}}".into();
    assert!(decode_case(&fixture).is_ok());
    fixture.text[21] = fixture.text[21].replace(
        "\"extensions\":{",
        "\"extensions\":{\"duplicate\":[1,2],\"duplicate\":{\"keep\":\"value\"},",
    );
    assert!(decode_case(&fixture).is_ok());
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn scalar_error_order_nullable_and_profile_priority_are_preserved() {
    let mut fixture = Fixture::ordinary();
    fixture.text[0] = "bad".into();
    fixture.text[15] = "bad".into();
    assert!(matches!(
        decode_case(&fixture),
        Err(Failure::Corrupt { tag: 1, .. })
    ));
    let mut fixture = Fixture::ordinary();
    fixture.text[16] = "bad".into();
    fixture.text[18] = "bad".into();
    assert!(matches!(
        decode_case(&fixture),
        Err(Failure::Corrupt { tag: 19, .. })
    ));
    let mut fixture = Fixture::ordinary();
    fixture.text[19] = "bad".into();
    fixture.text[21] = "bad".into();
    assert!(matches!(
        decode_case(&fixture),
        Err(Failure::Corrupt { tag: 22, .. })
    ));
    let mut fixture = Fixture::ordinary();
    fixture.connection.port = 0;
    fixture.text[3] = "-bad".into();
    assert_eq!(
        decode_case(&fixture),
        Err(Failure::Connection(
            domain_decode::validation::ConnectionValidation::InvalidHost
        ))
    );
    for source in [
        ProfileSource::Explicit,
        ProfileSource::Connection,
        ProfileSource::Default,
    ] {
        let mut fixture = Fixture::ordinary();
        fixture.connection.terminal_profile_id = if source == ProfileSource::Default {
            None
        } else {
            Some(fixture.profile.id)
        };
        fixture.connection.position = i64::MIN;
        let fields = fixture.fields(true);
        let mut selection = fixture.selection();
        selection.explicit_profile = if source == ProfileSource::Explicit {
            Some(fixture.profile.id)
        } else {
            None
        };
        let identity = identity(u64::MAX);
        let raw = RawFields::checked(
            &fields,
            identity,
            source,
            selection,
            no_reference(identity, selection),
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        let root = root();
        let mut calls = ExecutionCounts::default();
        let build = domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls)
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(build.snapshot.value().source, source);
        assert_eq!(build.snapshot.value().target.position, i64::MIN);
        assert_eq!(build.snapshot.value().revision, u64::MAX);
    }
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn quick_target_and_resolution_defaults_clamps_use_complete_owned_snapshot() {
    let mut fixture = Fixture::ordinary();
    let mut settings = fixture.profile.settings.clone();
    settings.terminal_type = "   ".into();
    settings.font_family.clear();
    settings.answerback.clear();
    settings.initial_cols = 0;
    settings.initial_rows = u16::MAX;
    settings.scrollback_lines = usize::MAX;
    settings.font_size = 1.0;
    fixture.text[21] = serde_json::to_string(&settings).unwrap_or_else(|error| panic!("{error}"));
    let expected = settings
        .resolve(&TerminalOverrides::default())
        .with_app_key_bindings(&fixture.app.key_bindings);
    let fields = fixture.fields(false);
    let selection = Selection {
        connection: None,
        explicit_profile: None,
        expected_transport: TransportKind::NativeSsh,
        quick: Some(QuickTarget {
            id: ConnectionId(Uuid::from_u128(4)),
            host: " fixture.invalid ",
            port: u16::MAX,
            username: " 用户 ",
            command: "  printf fixture  ",
        }),
    };
    let identity = identity(9);
    let raw = RawFields::checked(
        &fields,
        identity,
        ProfileSource::Default,
        selection,
        no_reference(identity, selection),
    )
    .unwrap_or_else(|error| panic!("{error:?}"));
    let root = root();
    let mut calls = ExecutionCounts::default();
    let audit = audit::AuditScope::start(&root);
    let build = domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(build.snapshot.value().resolved, expected);
    assert_eq!(build.snapshot.value().target.name, "用户@fixture.invalid");
    assert_eq!(
        build.snapshot.value().target.remote_command.as_deref(),
        Some("printf fixture")
    );
    assert!(!build.snapshot.value().saved);
    drop(build);
    verify_audit(
        &audit.finish(),
        owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing,
    );
}

#[test]
fn actual_snapshot_layout_matches_current_target() {
    assert_eq!(
        domain_decode::SnapshotBody::actual_layout(),
        Layout::new::<domain_decode::SnapshotBody>()
    );
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn certified_private_error_formula_covers_small_and_long_inputs() {
    let target = target();
    let private = target
        .private_bounds()
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(
        (private.error_code.size(), private.error_code.align()),
        (24, 8)
    );
    assert_eq!(
        (
            private.error_impl_actual.size(),
            private.error_impl_actual.align()
        ),
        (40, 8)
    );
    let ordinary = decode_shape::CostInputs::default();
    let long = decode_shape::CostInputs {
        max_string_scalars_upper: 400,
        max_decoded_string_bytes_upper: 1600,
        tokens_started: 10_000,
        ..ordinary
    };
    let small = domain_decode::error_bound::bound(ordinary, 1, &target)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let large = domain_decode::error_bound::bound(long, 10_000, &target)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert!(large.message_bytes > 4000);
    assert!(large.total > small.total);
}

#[test]
fn mismatched_target_rejects_before_schema_core_and_allocation() {
    let mut mismatch = certificate::GNU_1981;
    mismatch.json_feature_bits |= 4;
    let mismatch = certificate::TargetLayouts::foundation(mismatch)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let fixture = Fixture::ordinary();
    let fields = fixture.fields(true);
    let raw = raw(&fixture, &fields);
    let root = root();
    let mut calls = ExecutionCounts::default();
    let audit = audit::AuditScope::start(&root);
    assert!(matches!(
        domain_decode::decode_resolve(&raw, &mismatch, &root, ADMISSION, &mut calls),
        Err(Failure::Resource(
            layout::NumericReject::UnsupportedEnvelope
        ))
    ));
    assert_eq!(calls, ExecutionCounts::default());
    assert!(audit.finish().events().is_empty());
    let mut changed = target();
    changed.public.key_binding = Layout::new::<u8>();
    let audit = audit::AuditScope::start(&root);
    assert!(matches!(
        domain_decode::decode_resolve(&raw, &changed, &root, ADMISSION, &mut calls),
        Err(Failure::Resource(
            layout::NumericReject::UnsupportedEnvelope
        ))
    ));
    assert_eq!(calls, ExecutionCounts::default());
    assert!(audit.finish().events().is_empty());
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
#[cfg(not(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
)))]
fn uncertified_target_rejects_before_quota_schema_core_and_allocation() {
    let fixture = Fixture::ordinary();
    let fields = fixture.fields(true);
    let raw = raw(&fixture, &fields);
    let target = target();
    assert_eq!(
        target.private_bounds(),
        Err(layout::NumericReject::UnsupportedEnvelope)
    );
    for bytes in [0, BYTE_LIMIT] {
        let root = ProcessLedger::new(bytes, [u32::MAX; CLASS_COUNT]);
        let mut calls = ExecutionCounts::default();
        let audit = audit::AuditScope::start(&root);
        assert!(matches!(
            domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls),
            Err(Failure::Resource(
                layout::NumericReject::UnsupportedEnvelope
            ))
        ));
        assert_eq!(calls, ExecutionCounts::default());
        assert!(audit.finish().events().is_empty());
        assert_eq!(root.snapshot().rust_bytes, 0);
    }
}

/// 仅测试链接现有 bundled SQLite 的公开 C ABI；不新增依赖，不进入产品读路径。
mod storage_oracle {
    use super::{CatalogMutation, Fixture, GroupId, GroupPresence, SqliteRepository, Uuid};
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::path::PathBuf;

    unsafe extern "C" {
        fn sqlite3_open_v2(
            filename: *const c_char,
            database: *mut *mut c_void,
            flags: c_int,
            vfs: *const c_char,
        ) -> c_int;
        fn sqlite3_close(database: *mut c_void) -> c_int;
        fn sqlite3_prepare_v2(
            database: *mut c_void,
            sql: *const c_char,
            length: c_int,
            statement: *mut *mut c_void,
            tail: *mut *const c_char,
        ) -> c_int;
        fn sqlite3_bind_text(
            statement: *mut c_void,
            index: c_int,
            value: *const c_char,
            length: c_int,
            destructor: Option<unsafe extern "C" fn(*mut c_void)>,
        ) -> c_int;
        fn sqlite3_step(statement: *mut c_void) -> c_int;
        fn sqlite3_extended_errcode(database: *mut c_void) -> c_int;
        fn sqlite3_column_text(statement: *mut c_void, column: c_int) -> *const u8;
        fn sqlite3_column_bytes(statement: *mut c_void, column: c_int) -> c_int;
        fn sqlite3_finalize(statement: *mut c_void) -> c_int;
    }

    pub struct Oracle {
        pub repository: SqliteRepository,
        directory: PathBuf,
        path: PathBuf,
    }

    impl Oracle {
        pub fn new(fixture: &Fixture) -> Self {
            let parent = std::env::temp_dir();
            assert!(parent.is_dir(), "测试只使用预先存在的标准临时根");
            let directory = parent.join(format!("selected-domain-{}", Uuid::new_v4()));
            std::fs::create_dir(&directory)
                .unwrap_or_else(|error| panic!("创建合成 fixture 失败: {:?}", error.kind()));
            let path = directory.join("fixture.db");
            let repository =
                SqliteRepository::open(&path).unwrap_or_else(|error| panic!("{error:?}"));
            repository
                .migrate()
                .unwrap_or_else(|error| panic!("{error:?}"));
            repository
                .save_terminal_profile(fixture.profile.clone())
                .unwrap_or_else(|error| panic!("{error:?}"));
            repository
                .save_settings(fixture.app.clone())
                .unwrap_or_else(|error| panic!("{error:?}"));
            repository
                .apply(CatalogMutation::Create(fixture.connection.clone()))
                .unwrap_or_else(|error| panic!("{error:?}"));
            Self {
                repository,
                directory,
                path,
            }
        }

        pub fn replace(&self, tag: u16, value: &str) {
            let sql = match tag {
                2 => c"UPDATE connections SET group_id=?1",
                4 => c"UPDATE connections SET host=?1",
                7 => c"UPDATE connections SET transport=?1",
                16 => c"UPDATE connections SET terminal_overrides_json=?1",
                19 => c"UPDATE app_settings SET key_bindings_json=?1",
                22 => c"UPDATE terminal_profiles SET settings_json=?1 WHERE id='00000000-0000-0000-0000-000000000002'",
                _ => panic!("oracle 只修改指定合成字段"),
            };
            let path = CString::new(
                self.path
                    .to_str()
                    .unwrap_or_else(|| panic!("fixture 路径必须 UTF-8")),
            )
            .unwrap_or_else(|_| panic!("fixture 路径含 NUL"));
            let mut db = std::ptr::null_mut();
            let mut statement = std::ptr::null_mut();
            let length = c_int::try_from(value.len()).unwrap_or_else(|_| panic!("fixture 过长"));
            // SAFETY: 路径/SQL 为 NUL 结尾；out pointers 有效；value 从 bind 到 step/finalize
            // 始终借用存活，None destructor 对应 SQLITE_STATIC；每个 handle 恰好关闭一次。
            unsafe {
                assert_eq!(
                    sqlite3_open_v2(path.as_ptr(), &mut db, 2, std::ptr::null()),
                    0
                );
                if tag == 2 {
                    // 合成悬空/别名引用使用本次测试连接的 FK-off；不改变 repository worker。
                    let mut pragma = std::ptr::null_mut();
                    let prepare = sqlite3_prepare_v2(
                        db,
                        c"PRAGMA foreign_keys=OFF".as_ptr(),
                        -1,
                        &mut pragma,
                        std::ptr::null_mut(),
                    );
                    if prepare != 0 {
                        assert_eq!(sqlite3_close(db), 0);
                        panic!("oracle pragma prepare code {prepare}");
                    }
                    let step = sqlite3_step(pragma);
                    let finalize = sqlite3_finalize(pragma);
                    if step != 101 || finalize != 0 {
                        assert_eq!(sqlite3_close(db), 0);
                        panic!("oracle pragma codes {step}/{finalize}");
                    }
                }
                let prepare =
                    sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut statement, std::ptr::null_mut());
                if prepare != 0 {
                    assert_eq!(sqlite3_close(db), 0);
                    panic!("oracle prepare code {prepare}");
                }
                let bind = sqlite3_bind_text(statement, 1, value.as_ptr().cast(), length, None);
                let step = if bind == 0 {
                    sqlite3_step(statement)
                } else {
                    bind
                };
                let extended = sqlite3_extended_errcode(db);
                let finalize = sqlite3_finalize(statement);
                let close = sqlite3_close(db);
                assert_eq!(bind, 0);
                assert_eq!(step, 101, "oracle extended SQLite code {extended}");
                assert_eq!(finalize, 0);
                assert_eq!(close, 0);
            }
        }

        /// 事实只在测试从当前合成 DB 读取；不把文本 SQL 相等当 UUID 二进制相等。
        pub fn group_presence(&self, wanted: GroupId) -> GroupPresence {
            let path = CString::new(
                self.path
                    .to_str()
                    .unwrap_or_else(|| panic!("fixture UTF-8 路径")),
            )
            .unwrap_or_else(|_| panic!("fixture 路径含 NUL"));
            let mut db = std::ptr::null_mut();
            let mut statement = std::ptr::null_mut();
            let sql = c"SELECT id FROM connection_groups ORDER BY id";
            let mut exists = false;
            // SAFETY: readonly handle 和 statement 在本函数内唯一持有；column slice 只借用到
            // 下一 step/finalize 前；返回事实后不保留任何 SQLite pointer。
            unsafe {
                assert_eq!(
                    sqlite3_open_v2(path.as_ptr(), &mut db, 1, std::ptr::null()),
                    0
                );
                let prepare =
                    sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut statement, std::ptr::null_mut());
                if prepare != 0 {
                    assert_eq!(sqlite3_close(db), 0);
                    panic!("oracle group prepare code {prepare}");
                }
                let terminal = loop {
                    let code = sqlite3_step(statement);
                    if code != 100 {
                        break code;
                    }
                    let pointer = sqlite3_column_text(statement, 0);
                    let length = sqlite3_column_bytes(statement, 0);
                    assert!(!pointer.is_null() && length >= 0);
                    let bytes = std::slice::from_raw_parts(pointer, length as usize);
                    let text = std::str::from_utf8(bytes)
                        .unwrap_or_else(|_| panic!("fixture group UTF-8"));
                    let id = GroupId(
                        Uuid::parse_str(text).unwrap_or_else(|_| panic!("fixture group UUID")),
                    );
                    exists |= id == wanted;
                };
                let finalize = sqlite3_finalize(statement);
                let close = sqlite3_close(db);
                assert_eq!(terminal, 101);
                assert_eq!(finalize, 0);
                assert_eq!(close, 0);
            }
            if exists {
                GroupPresence::Exists(wanted)
            } else {
                GroupPresence::Absent(wanted)
            }
        }
    }

    impl Drop for Oracle {
        fn drop(&mut self) {
            let shutdown = self.repository.shutdown();
            assert!(shutdown.is_ok(), "oracle worker 必须先退出");
            for name in ["fixture.db", "fixture.db-wal", "fixture.db-shm"] {
                match std::fs::remove_file(self.directory.join(name)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("fixture 清理失败: {:?}", error.kind()),
                }
            }
            // 非递归删除；意外文件会使清理失败，不能扩大删除范围。
            std::fs::remove_dir(&self.directory)
                .unwrap_or_else(|error| panic!("fixture 目录清理失败: {:?}", error.kind()));
        }
    }
}

#[test]
fn actual_private_storage_codec_acceptance_matches_wrappers_and_complete_typed_values() {
    use rshell_m0::rshell_storage::StorageError;
    let fixture = Fixture::ordinary();
    let oracle = storage_oracle::Oracle::new(&fixture);
    let stored = oracle
        .repository
        .load_catalog()
        .unwrap_or_else(|error| panic!("{error:?}"));
    let connection = stored
        .connections
        .get(&fixture.connection.id)
        .unwrap_or_else(|| panic!("oracle connection"));
    let fixture = Fixture::from_values(
        connection.clone(),
        fixture.profile.clone(),
        fixture.app.clone(),
    );
    for (tag, cases) in [
        (16, vec!["{}".into(), "[]".into(), "{\"version\":0}".into(), "{\"version\":1,\"version\":1}".into(),
            "{\"version\":1,\"font_family\":\"a\",\"font_family\":\"b\"}".into(),
            "{\"version\":1,\"unknown\":{\"x\":[\"🦀\\u0000\",true,null]}}".into(),
            "{\"version\":1,\"key_bindings\":[[\"enter\",[false,false,false,false],\"new_tab\"]]}".into(),
            "{\"version\":1,\"color_scheme\":\"unknown\"}".into(), "{\"version\":1,\"key_bindings\":[{".into()]),
        (19, vec!["{}".into(), "[1,[]]".into(), "[2,[]]".into(), "{\"version\":1,\"key_bindings\":[]}".into(),
            "{\"version\":1,\"key_bindings\":[[\"enter\",[false,false,false,false],\"new_tab\"]]}".into(),
            "{\"version\":1,\"key_bindings\":[{\"code\":{\"character\":\"\\u0000\"},\"modifiers\":[false,false,false,false],\"action\":\"new_tab\"}]}".into(),
            "{\"version\":1,\"key_bindings\":[{\"code\":{\"f\":256},\"modifiers\":[false,false,false,false],\"action\":\"new_tab\"}]}".into()]),
        (22, vec![fixture.text[21].clone(), "{\"version\":2}".into(), "{\"version\":1,\"initial_cols\":65536}".into(),
            format!("{}{}{}", &fixture.text[21][..fixture.text[21].len()-1], ",\"ignored\":", format_args!("{}\"\\uD800\"{}}}", "[".repeat(180), "]".repeat(180))),
            fixture.text[21].replace("\"extensions\":{", "\"extensions\":{\"same\":[],\"same\":{\"v\":\"🦀\"},")]),
    ] {
        for (index, text) in cases.into_iter().enumerate() {
            let accepted = match tag {
                16 => matches!(index, 5 | 6),
                19 => matches!(index, 1 | 3 | 4 | 5),
                _ => matches!(index, 0 | 3 | 4),
            };
            let mut fixture = Fixture::from_values(fixture.connection.clone(), fixture.profile.clone(), fixture.app.clone());
            fixture.text[usize::from(tag-1)] = text.clone();
            oracle.replace(16, &Fixture::ordinary().text[15]);
            oracle.replace(19, &Fixture::ordinary().text[18]);
            oracle.replace(22, &Fixture::ordinary().text[21]);
            oracle.replace(tag, &text);
            let fields = fixture.fields(true); let raw = raw(&fixture, &fields); let root = root(); let target = target();
            let mut calls = ExecutionCounts::default();
            // 旧 codec 行为在所有平台核验；新完整图的预算比较仅使用获证 GNU 布局。
            let new = cfg!(all(target_os = "windows", target_env = "gnu", target_pointer_width = "64", target_endian = "little", panic = "unwind"))
                .then(|| domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls));
            match tag {
                16 => match oracle.repository.load_catalog() {
                    Ok(old) => {
                        assert!(accepted);
                        let old = old.connections.get(&fixture.connection.id).unwrap_or_else(|| panic!("oracle connection"));
                        if let Some(new) = new {
                            let new = new.unwrap_or_else(|error| panic!("{error:?}"));
                            assert_eq!(&new.snapshot.value().target, old);
                        }
                    }
                    Err(StorageError::Corrupt) => {
                        assert!(!accepted);
                        if let Some(new) = new {
                            assert!(matches!(new, Err(Failure::Corrupt { .. } | Failure::Connection(_) | Failure::Settings(_))));
                        }
                    }
                    Err(error) => panic!("{error:?}"),
                },
                19 => match oracle.repository.load_settings() {
                    Ok(old) => {
                        assert!(accepted);
                        if let Some(new) = new {
                            assert_eq!(new.unwrap_or_else(|error| panic!("{error:?}")).snapshot.value().app, old);
                        }
                    }
                    Err(StorageError::Corrupt) => {
                        assert!(!accepted);
                        if let Some(new) = new {
                            assert!(matches!(new, Err(Failure::Corrupt { tag: 19, .. })));
                        }
                    }
                    Err(error) => panic!("{error:?}"),
                },
                _ => match oracle.repository.load_terminal_profiles() {
                    Ok(old) => {
                        assert!(accepted);
                        let old = old.iter().find(|profile| profile.id == fixture.profile.id).unwrap_or_else(|| panic!("oracle profile"));
                        if let Some(new) = new {
                            assert_eq!(&new.unwrap_or_else(|error| panic!("{error:?}")).snapshot.value().profile, old);
                        }
                    }
                    Err(StorageError::Corrupt) => {
                        assert!(!accepted);
                        if let Some(new) = new {
                            assert!(matches!(new, Err(Failure::Corrupt { tag: 22, .. })));
                        }
                    }
                    Err(error) => panic!("{error:?}"),
                },
            }
        }
    }
}

#[test]
fn storage_group_presence_uses_binary_uuid_identity() {
    let fixture = Fixture::ordinary();
    let oracle = storage_oracle::Oracle::new(&fixture);
    let group = GroupId(Uuid::from_u128(0xabcdef));
    assert_eq!(oracle.group_presence(group), GroupPresence::Absent(group));
    oracle
        .repository
        .apply(CatalogMutation::CreateGroup(ConnectionGroup {
            id: group,
            parent_id: None,
            name: "合成组".into(),
            position: 0,
        }))
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(oracle.group_presence(group), GroupPresence::Exists(group));
    for alias in [
        group.0.to_string(),
        group.0.simple().to_string(),
        group.0.to_string().to_uppercase(),
        format!("{{{}}}", group.0),
        group.0.urn().to_string(),
    ] {
        oracle.replace(2, &alias);
        let catalog = oracle
            .repository
            .load_catalog()
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(
            catalog
                .connections
                .get(&fixture.connection.id)
                .unwrap_or_else(|| panic!("oracle connection"))
                .group_id,
            Some(group)
        );
    }
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn long_error_templates_numeric_boundaries_and_allocation_failure_are_audited() {
    let mut texts = Vec::new();
    for value in [
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!(u64::MAX),
        serde_json::json!(i64::MIN),
        serde_json::json!(1.5),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!("\0🦀".repeat(800)),
    ] {
        let mut fixture = Fixture::ordinary();
        let mut settings: serde_json::Value =
            serde_json::from_str(&fixture.text[21]).unwrap_or_else(|error| panic!("{error}"));
        settings["color_scheme"] = value;
        fixture.text[21] =
            serde_json::to_string(&settings).unwrap_or_else(|error| panic!("{error}"));
        texts.push(fixture.text[21].clone());
        assert!(matches!(
            decode_case(&fixture),
            Err(Failure::Corrupt { tag: 22, .. })
        ));
    }
    for text in texts {
        let old = serde_json::from_str::<TerminalSettingsV1>(&text)
            .err()
            .unwrap_or_else(|| panic!("必须是真实 typed 错误"));
        let mut fixture = Fixture::ordinary();
        fixture.text[21] = text;
        let result = decode_case(&fixture);
        let category = match old.classify() {
            serde_json::error::Category::Io => 1,
            serde_json::error::Category::Syntax => 2,
            serde_json::error::Category::Data => 3,
            serde_json::error::Category::Eof => 4,
        };
        assert!(
            matches!(result, Err(Failure::Corrupt { json: Some(error), .. }) if error.category == category && error.line == old.line() && error.column == old.column())
        );
    }
    let fixture = Fixture::ordinary();
    let fields = fixture.fields(true);
    let raw = raw(&fixture, &fields);
    let target = target();
    let requirement = domain_decode::budget::scratch_requirement(&raw)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let root = root();
    let mut calls = ExecutionCounts::default();
    let audit = audit::AuditScope::start(&root);
    audit.fail_next_size(requirement.bytes);
    assert!(matches!(
        domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls),
        Err(Failure::Resource(layout::NumericReject::AllocationFailure))
    ));
    let report = audit.finish();
    assert_eq!(report.events().len(), 1);
    assert_eq!(report.events()[0].kind, 4);
    assert_eq!(calls, ExecutionCounts::default());
    assert_eq!(root.snapshot().rust_bytes, 0);
    let audit = audit::AuditScope::start(&root);
    let owner_layout = owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
        .unwrap_or_else(|error| panic!("{error:?}"));
    audit.fail_next_size(owner_layout.backing.size());
    assert!(matches!(
        domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls),
        Err(Failure::Resource(layout::NumericReject::AllocationFailure))
    ));
    verify_audit(&audit.finish(), owner_layout.backing);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn real_raw_owner_and_decode_grants_coexist_until_actual_backing_drop() {
    #[repr(C)]
    struct RawMetadata {
        revision: u64,
        field_count: usize,
    }
    let fixture = Fixture::ordinary();
    let fields = fixture.fields(true);
    let bytes: usize = fields
        .iter()
        .filter_map(|field| {
            if let FieldValue::Text(text) = field.value {
                Some(text.len())
            } else {
                None
            }
        })
        .sum();
    let root = root();
    let raw_layout = owner::OwnerLayout::actual::<RawMetadata>(bytes)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let grant = root
        .try_reserve(
            ledger::Amount {
                rust_bytes: raw_layout.backing.size(),
                c_bytes: 0,
                slots: 1,
            },
            ledger::ResourceClass::Raw,
            ADMISSION,
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
    let raw_owner = owner::SharedOwner::try_init(grant, bytes, 0, |payload| {
        let mut offset = 0;
        for field in &fields {
            if let FieldValue::Text(text) = field.value {
                payload[offset..offset + text.len()].copy_from_slice(text.as_bytes());
                offset += text.len();
            }
        }
        Ok(RawMetadata {
            revision: 17,
            field_count: fields.len(),
        })
    })
    .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(raw_owner.value().field_count, 24);
    let mut offset = 0;
    let borrowed: [RawField<'_>; 24] = std::array::from_fn(|index| {
        let mut field = fields[index];
        if let FieldValue::Text(text) = field.value {
            field.value = FieldValue::Text(
                std::str::from_utf8(&raw_owner.payload()[offset..offset + text.len()])
                    .unwrap_or_else(|_| panic!("fixture UTF-8")),
            );
            offset += text.len();
        }
        field
    });
    let raw = RawFields::checked(
        &borrowed,
        identity(raw_owner.value().revision),
        ProfileSource::Connection,
        fixture.selection(),
        no_reference(identity(raw_owner.value().revision), fixture.selection()),
    )
    .unwrap_or_else(|error| panic!("{error:?}"));
    let mut calls = ExecutionCounts::default();
    let build = domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(
        root.snapshot().classes[ledger::ResourceClass::Raw as usize].rust_bytes,
        raw_layout.backing.size()
    );
    assert!(root.snapshot().classes[ledger::ResourceClass::Decode as usize].rust_bytes > 0);
    drop(raw_owner);
    assert_eq!(
        root.snapshot().classes[ledger::ResourceClass::Raw as usize].rust_bytes,
        0
    );
    assert_eq!(build.snapshot.value().revision, 17);
    drop(build);
    assert_eq!(root.snapshot().rust_bytes, 0);
}

#[test]
fn selected_connection_validator_matches_actual_catalog_predicates_without_heap() {
    use domain_decode::validation::ConnectionValidation as V;
    use rshell_m0::rshell_core::{ConnectionCatalog, DomainError};
    for host in ["", "  ", "-bad", "fixture.invalid", "two words"] {
        for port in [0, 1, u16::MAX] {
            for transport in [TransportKind::NativeSsh, TransportKind::SystemOpenSsh] {
                for authentication in [
                    AuthenticationKind::Password,
                    AuthenticationKind::PublicKey,
                    AuthenticationKind::Agent,
                    AuthenticationKind::KeyboardInteractive,
                ] {
                    for path in [None, Some(""), Some("  "), Some("目录/🦀\0key")] {
                        let mut profile = Fixture::ordinary().connection;
                        profile.host = host.into();
                        profile.port = port;
                        profile.position = 0;
                        profile.transport = transport;
                        profile.authentication = authentication;
                        profile.identity_file = path.map(std::path::PathBuf::from);
                        let mut catalog = ConnectionCatalog::default();
                        catalog.connections.insert(profile.id, profile.clone());
                        let expected = catalog.validate().map_err(|error| match error {
                            DomainError::InvalidHost { .. } => Failure::Connection(V::InvalidHost),
                            DomainError::InvalidPort { .. } => Failure::Connection(V::InvalidPort),
                            DomainError::InvalidAuthentication { .. } => {
                                Failure::Connection(V::InvalidAuthentication)
                            }
                            DomainError::MissingIdentityFile { .. } => {
                                Failure::Connection(V::MissingIdentityFile)
                            }
                            DomainError::InvalidTerminalOverride { field, code } => {
                                Failure::Settings(rshell_m0::rshell_core::SettingsValidationError {
                                    field,
                                    code,
                                })
                            }
                            _ => panic!("fixture 未引入目录引用/顺序错误"),
                        });
                        let root = root();
                        let audit = audit::AuditScope::start(&root);
                        assert_eq!(domain_decode::validation::connection(&profile), expected);
                        assert!(audit.finish().events().is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn raw_schema_nullable_order_and_explicit_selection_are_checked_without_decode() {
    let mut fixture = Fixture::ordinary();
    fixture.connection.terminal_profile_id = Some(TerminalProfileId(Uuid::from_u128(99)));
    fixture.connection.credential_ref = None;
    fixture.connection.identity_file = None;
    fixture.connection.remote_command = None;
    fixture.text[14] = fixture
        .connection
        .terminal_profile_id
        .map(|id| id.0.to_string())
        .unwrap_or_default();
    let fields = fixture.fields(true);
    let mut selection = fixture.selection();
    selection.explicit_profile = Some(fixture.profile.id);
    let identity = identity(9);
    let raw = RawFields::checked(
        &fields,
        identity,
        ProfileSource::Explicit,
        selection,
        no_reference(identity, selection),
    )
    .unwrap_or_else(|error| panic!("{error:?}"));
    let root = root();
    assert_eq!(raw.source, ProfileSource::Explicit);
    assert_eq!(raw.selection.explicit_profile, Some(fixture.profile.id));
    if cfg!(all(
        target_os = "windows",
        target_env = "gnu",
        target_pointer_width = "64",
        target_endian = "little",
        panic = "unwind"
    )) {
        let mut calls = ExecutionCounts::default();
        let build = domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls)
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(build.snapshot.value().profile.id, fixture.profile.id);
        assert_eq!(
            build.snapshot.value().target.terminal_profile_id,
            fixture.connection.terminal_profile_id
        );
        assert!(build.snapshot.value().target.identity_file.is_none());
        drop(build);
        selection.explicit_profile = Some(TerminalProfileId(Uuid::from_u128(100)));
        let raw = RawFields::checked(
            &fields,
            identity,
            ProfileSource::Explicit,
            selection,
            no_reference(identity, selection),
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        assert!(matches!(
            domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls),
            Err(Failure::MissingProfile)
        ));
    }
    for mutation in 0..6 {
        let mut fields = fixture.fields(true);
        match mutation {
            0 => fields[0].value = FieldValue::Null,
            1 => fields[4].value = FieldValue::Text("22"),
            2 => fields[13].value = FieldValue::Null,
            3 => fields[1].tag = 1,
            4 => fields[22].ordinal = 2,
            _ => fields[23].value = FieldValue::Text("a"),
        }
        let audit = audit::AuditScope::start(&root);
        assert!(matches!(
            RawFields::checked(
                &fields,
                identity,
                ProfileSource::Explicit,
                selection,
                no_reference(identity, selection)
            ),
            Err(Failure::Protocol)
        ));
        assert!(audit.finish().events().is_empty());
    }
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn complete_resolution_preserves_profile_shadowing_app_order_and_typed_depth_errors() {
    let fixture = Fixture::ordinary();
    let mut connection = fixture.connection;
    connection.terminal_overrides.key_bindings = None;
    let app = AppSettings {
        key_bindings: vec![
            binding(KeyCode::Enter, "split_vertical"),
            binding(KeyCode::F(1), "new_tab"),
            binding(KeyCode::F(1), "clear_scrollback"),
            binding(KeyCode::Tab, "split_vertical"),
        ],
        ..fixture.app
    };
    let mut fixture = Fixture::from_values(connection, fixture.profile, app);
    let expected = fixture
        .profile
        .settings
        .resolve(&fixture.connection.terminal_overrides)
        .with_app_key_bindings(&fixture.app.key_bindings);
    let fields = fixture.fields(true);
    let raw = raw(&fixture, &fields);
    let root = root();
    let mut calls = ExecutionCounts::default();
    let build = domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(build.snapshot.value().resolved, expected);
    assert_eq!(build.snapshot.value().resolved.key_bindings.len(), 3);
    let zero = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
    let audit = audit::AuditScope::start(&zero);
    let value = domain_decode::clone_snapshot(build.snapshot.value(), &target(), &zero, ADMISSION);
    assert!(matches!(
        value,
        Err(Failure::Resource(layout::NumericReject::RustBudgetExceeded))
    ));
    let report = audit.finish();
    assert!(report.events().is_empty());
    drop(build);
    fixture.text[21] = fixture.text[21].replace(
        "\"extensions\":{",
        &format!(
            "\"extensions\":{{\"deep\":{}0{},",
            "[".repeat(180),
            "]".repeat(180)
        ),
    );
    assert!(matches!(
        decode_case(&fixture),
        Err(Failure::Corrupt { tag: 22, .. })
    ));
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn explicit_group_facts_match_real_storage_core_and_binary_uuid_aliases() {
    use rshell_m0::rshell_core::DomainError;
    use rshell_m0::rshell_storage::StorageError;
    let group = GroupId(Uuid::from_u128(0xabcdef));
    let canonical = group.0.to_string();
    let aliases = [
        canonical.clone(),
        group.0.simple().to_string(),
        canonical.to_uppercase(),
        format!("{{{canonical}}}"),
        group.0.urn().to_string(),
    ];
    for exists in [false, true] {
        let ordinary = Fixture::ordinary();
        let oracle = storage_oracle::Oracle::new(&ordinary);
        if exists {
            oracle
                .repository
                .apply(CatalogMutation::CreateGroup(ConnectionGroup {
                    id: group,
                    parent_id: None,
                    name: "合成组".into(),
                    position: 0,
                }))
                .unwrap_or_else(|error| panic!("{error:?}"));
        }
        let baseline = oracle
            .repository
            .load_catalog()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let configuration = oracle
            .repository
            .load_configuration()
            .unwrap_or_else(|error| panic!("{error:?}"));
        let snapshot_identity = identity(configuration.revision);
        let mut connection = baseline
            .connections
            .get(&ordinary.connection.id)
            .unwrap_or_else(|| panic!("fixture connection"))
            .clone();
        connection.group_id = Some(group);
        for alias in &aliases {
            for fault in 0..6 {
                let mut fixture = Fixture::from_values(
                    connection.clone(),
                    ordinary.profile.clone(),
                    ordinary.app.clone(),
                );
                fixture.text[1] = alias.clone();
                match fault {
                    1 => fixture.text[15] = "{\"version\":1,\"font_family\":\"unfinished".into(),
                    2 => fixture.text[15] = "{\"version\":2}".into(),
                    3 => {
                        fixture.text[3] = "-bad".into();
                        fixture.connection.host = "-bad".into();
                    }
                    4 => {
                        fixture.text[15] = "{\"version\":1,\"initial_cols\":0}".into();
                        fixture.connection.terminal_overrides = TerminalOverrides {
                            initial_cols: Some(0),
                            ..Default::default()
                        };
                    }
                    5 => fixture.text[6] = "invalid-transport".into(),
                    _ => {}
                }
                oracle.replace(2, &fixture.text[1]);
                oracle.replace(4, &fixture.text[3]);
                oracle.replace(7, &fixture.text[6]);
                oracle.replace(16, &fixture.text[15]);
                let presence = oracle.group_presence(group);
                assert_eq!(
                    presence,
                    if exists {
                        GroupPresence::Exists(group)
                    } else {
                        GroupPresence::Absent(group)
                    }
                );
                let selection = fixture.selection();
                let references = SelectedReferences::from_trusted_transaction(
                    SelectedReferenceToken {
                        snapshot: snapshot_identity,
                        selected_connection: selection.connection,
                    },
                    presence,
                );
                let fields = fixture.fields(true);
                let raw = RawFields::checked(
                    &fields,
                    snapshot_identity,
                    ProfileSource::Connection,
                    selection,
                    references,
                )
                .unwrap_or_else(|error| panic!("{error:?}"));
                let root = root();
                let mut calls = ExecutionCounts::default();
                let audit = audit::AuditScope::start(&root);
                let new =
                    domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls);
                let result = new.map(|build| {
                    assert_eq!(build.snapshot.value().target.group_id, Some(group));
                    assert_eq!(build.snapshot.value().selected_references, references);
                    drop(build);
                });
                verify_audit(
                    &audit.finish(),
                    owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
                        .unwrap_or_else(|error| panic!("{error:?}"))
                        .backing,
                );
                assert_eq!(root.snapshot().rust_bytes, 0);
                if matches!(fault, 1 | 2 | 5) {
                    let expected_tag = if fault == 5 { 7 } else { 16 };
                    assert!(
                        matches!(result, Err(Failure::Corrupt { tag, .. }) if tag == expected_tag)
                    );
                } else {
                    let mut catalog = baseline.clone();
                    catalog
                        .connections
                        .insert(fixture.connection.id, fixture.connection.clone());
                    let core = catalog.validate().map_err(|error| match error {
                        DomainError::GroupNotFound(id) => Failure::MissingGroup(id),
                        DomainError::InvalidHost { .. } => Failure::Connection(
                            domain_decode::validation::ConnectionValidation::InvalidHost,
                        ),
                        DomainError::InvalidTerminalOverride { field, code } => {
                            Failure::Settings(rshell_m0::rshell_core::SettingsValidationError {
                                field,
                                code,
                            })
                        }
                        _ => panic!("fixture 不引入其他目录规则"),
                    });
                    assert_eq!(result, core);
                }
                match oracle.repository.load_catalog() {
                    Ok(old) => {
                        assert!(result.is_ok());
                        let mut calls = ExecutionCounts::default();
                        let build = domain_decode::decode_resolve(
                            &raw,
                            &target(),
                            &root,
                            ADMISSION,
                            &mut calls,
                        )
                        .unwrap_or_else(|error| panic!("{error:?}"));
                        assert_eq!(
                            &build.snapshot.value().target,
                            old.connections
                                .get(&fixture.connection.id)
                                .unwrap_or_else(|| panic!("oracle connection"))
                        );
                        assert_eq!(calls, ExecutionCounts { schema: 3, core: 1 });
                    }
                    Err(StorageError::Corrupt) => assert!(result.is_err()),
                    Err(error) => panic!("{error:?}"),
                }
            }
        }
    }
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn unproven_or_inconsistent_group_facts_reject_at_validation_and_quota_still_precedes_codec() {
    let mut fixture = Fixture::ordinary();
    let group = GroupId(Uuid::from_u128(20));
    fixture.connection.group_id = Some(group);
    fixture.text[1] = group.0.to_string();
    let snapshot_identity = identity(7);
    let selection = fixture.selection();
    let token = SelectedReferenceToken {
        snapshot: snapshot_identity,
        selected_connection: selection.connection,
    };
    for (token, presence, expected) in [
        (token, GroupPresence::Unproven, Failure::ReferenceNotReady),
        (token, GroupPresence::NoReference, Failure::Protocol),
        (
            token,
            GroupPresence::Exists(GroupId(Uuid::from_u128(21))),
            Failure::Protocol,
        ),
        (
            token,
            GroupPresence::Absent(GroupId(Uuid::from_u128(21))),
            Failure::Protocol,
        ),
        (
            SelectedReferenceToken {
                snapshot: RawIdentity {
                    transaction_nonce: [12; 16],
                    ..snapshot_identity
                },
                ..token
            },
            GroupPresence::Exists(group),
            Failure::Protocol,
        ),
        (
            SelectedReferenceToken {
                snapshot: RawIdentity {
                    revision: 8,
                    ..snapshot_identity
                },
                ..token
            },
            GroupPresence::Exists(group),
            Failure::Protocol,
        ),
        (
            SelectedReferenceToken {
                selected_connection: Some(ConnectionId(Uuid::from_u128(4))),
                ..token
            },
            GroupPresence::Exists(group),
            Failure::Protocol,
        ),
    ] {
        let fields = fixture.fields(true);
        let root = root();
        let mut calls = ExecutionCounts::default();
        let references = SelectedReferences::from_trusted_transaction(token, presence);
        let audit = audit::AuditScope::start(&root);
        let raw = RawFields::checked(
            &fields,
            snapshot_identity,
            ProfileSource::Connection,
            selection,
            references,
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        assert!(audit.finish().events().is_empty());
        let result =
            domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls).map(drop);
        assert_eq!(result, Err(expected));
        assert_eq!(calls, ExecutionCounts { schema: 1, core: 0 });
    }
    for presence in [GroupPresence::Unproven, GroupPresence::Absent(group)] {
        let references = SelectedReferences::from_trusted_transaction(token, presence);
        let fields = fixture.fields(true);
        let raw = RawFields::checked(
            &fields,
            snapshot_identity,
            ProfileSource::Connection,
            selection,
            references,
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        let zero = ProcessLedger::new(0, [u32::MAX; CLASS_COUNT]);
        let mut calls = ExecutionCounts::default();
        let audit = audit::AuditScope::start(&zero);
        assert!(matches!(
            domain_decode::decode_resolve(&raw, &target(), &zero, ADMISSION, &mut calls),
            Err(Failure::Resource(layout::NumericReject::RustBudgetExceeded))
        ));
        assert!(audit.finish().events().is_empty());
        assert_eq!(calls, ExecutionCounts::default());
    }
    fixture.connection.group_id = None;
    for presence in [
        GroupPresence::Exists(group),
        GroupPresence::Absent(group),
        GroupPresence::Unproven,
    ] {
        let fields = fixture.fields(true);
        let references = SelectedReferences::from_trusted_transaction(token, presence);
        let raw = RawFields::checked(
            &fields,
            snapshot_identity,
            ProfileSource::Connection,
            selection,
            references,
        )
        .unwrap_or_else(|error| panic!("{error:?}"));
        let root = root();
        let mut calls = ExecutionCounts::default();
        let result =
            domain_decode::decode_resolve(&raw, &target(), &root, ADMISSION, &mut calls).map(drop);
        assert_eq!(
            result,
            Err(if presence == GroupPresence::Unproven {
                Failure::ReferenceNotReady
            } else {
                Failure::Protocol
            })
        );
    }
}

#[test]
#[cfg(all(
    target_os = "windows",
    target_env = "gnu",
    target_pointer_width = "64",
    target_endian = "little",
    panic = "unwind"
))]
fn split_boundaries_and_long_nested_arrays_survive_tighten_clone_and_last_drop_audit() {
    use serde_json::Value;
    for count in [11, 12, 13] {
        let fixture = Fixture::ordinary();
        let mut connection = fixture.connection;
        let mut profile = fixture.profile;
        connection.tags = (0..count)
            .map(|index| format!("tag-{index:02}-🦀"))
            .collect();
        profile.settings.extensions.clear();
        for index in 0..count {
            let value = if index == 0 {
                Value::Array(vec![Value::Array(
                    (0..24)
                        .map(|part| {
                            Value::String(format!("nested-{part:02}-{}\0", "🦀".repeat(24)))
                        })
                        .collect(),
                )])
            } else {
                Value::String(format!("extension-{index:02}"))
            };
            profile
                .settings
                .extensions
                .insert(format!("key-{index:02}"), value);
        }
        let expected = profile
            .settings
            .resolve(&connection.terminal_overrides)
            .with_app_key_bindings(&fixture.app.key_bindings);
        let fixture = Fixture::from_values(connection, profile, fixture.app);
        let fields = fixture.fields(true);
        let raw = raw(&fixture, &fields);
        let target = target();
        let root = root();
        let owner_layout = owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing;
        let mut calls = ExecutionCounts::default();
        let audit = audit::AuditScope::start(&root);
        let build = domain_decode::decode_resolve(&raw, &target, &root, ADMISSION, &mut calls)
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(build.snapshot.value().resolved, expected);
        assert_eq!(
            build.snapshot.admitted().rust_bytes,
            owner_layout.size() + build.clone_shape.retained_bytes
        );
        assert!(build.clone_shape.retained_bytes > build.clone_shape.clone_bytes);
        let report = audit.finish();
        let mut live = verify_audit_window(&report, owner_layout, 0);
        if count > 11 {
            let nodes = target
                .private_bounds()
                .unwrap_or_else(|error| panic!("{error:?}"));
            for bounds in [nodes.string_set_nodes, nodes.string_value_nodes] {
                assert!(report.events().iter().any(|event| event.kind == 1
                    && event.align == bounds.internal.align()
                    && event.size > bounds.leaf.size()
                    && event.size <= bounds.internal.size()));
            }
        }
        let audit = audit::AuditScope::start(&root);
        let copy = domain_decode::clone_snapshot(build.snapshot.value(), &target, &root, ADMISSION)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let shared = build
            .snapshot
            .try_clone()
            .unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(copy.value(), build.snapshot.value());
        let report = audit.finish();
        live = verify_audit_window(&report, owner_layout, live);
        let audit = audit::AuditScope::start(&root);
        drop(build);
        drop(copy);
        assert!(root.snapshot().rust_bytes > 0);
        assert_eq!(shared.value().resolved, expected);
        drop(shared);
        let report = audit.finish();
        live = verify_audit_window(&report, owner_layout, live);
        assert_eq!(live, 0);
        assert_eq!(root.snapshot().rust_bytes, 0);
    }
    println!(
        "RawIdentity={:?} SelectedReferenceToken={:?} GroupPresence={:?} SelectedReferences={:?} RawFields={:?} SnapshotBody={:?} SnapshotOwner={:?}",
        Layout::new::<RawIdentity>(),
        Layout::new::<SelectedReferenceToken>(),
        Layout::new::<GroupPresence>(),
        Layout::new::<SelectedReferences>(),
        Layout::new::<RawFields<'_>>(),
        domain_decode::SnapshotBody::actual_layout(),
        owner::OwnerLayout::actual::<domain_decode::SnapshotBody>(0)
            .unwrap_or_else(|error| panic!("{error:?}"))
            .backing
    );
}
