//! 字体文件、窗口外观、旧终端字段补丁与桌面配置事务的界面协议。

use rinf::{DartSignal, DartSignalBinary, RustSignal, RustSignalBinary, SignalPiece};
use serde::{Deserialize, Serialize};

/// 取当前设置。
#[derive(Deserialize, DartSignal)]
pub struct SettingsQuery {}

/// action 为 query、preview、apply 或 reset；字体文件经二进制通道传递。
#[derive(Deserialize, DartSignalBinary)]
pub struct FontFileRequest {
    pub request_id: u32,
    pub slot: String,
    pub action: String,
    pub file_name: String,
}

#[derive(Serialize, RustSignalBinary)]
pub struct FontFileState {
    pub request_id: u32,
    pub slot: String,
    pub family: String,
    pub label: String,
    pub error: String,
    pub applied: bool,
}

/// Windows 外观独立于终端配置，避免拖动透明度时覆盖其他设置。
/// request_id 由 Dart 分配并在 WindowsAppearanceState 中原样回传；查询使用 0。
#[derive(Deserialize, DartSignal)]
pub struct WindowsAppearanceQuery {
    pub request_id: u32,
}

#[derive(Deserialize, DartSignal)]
pub struct SaveWindowsAppearance {
    pub request_id: u32,
    pub acrylic: bool,
    pub opacity: f64,
}

#[derive(Serialize, RustSignal)]
pub struct WindowsAppearanceState {
    /// 对应请求的 request_id；保存结果只按它关联。
    pub request_id: u32,
    pub acrylic: bool,
    pub opacity: f64,
    /// 保存失败时返回原值与错误，界面撤销预览。
    pub error: String,
}

/// 只提交被修改的字段，避免尚未收到旧回包时覆盖其他设置。
#[derive(Default, Deserialize, DartSignal)]
pub struct SaveSettings {
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub scrollback_lines: Option<u32>,
    pub show_key_bar: Option<bool>,
}

/// 键位条排布独立保存，避免覆盖同时修改的字体和滚回设置。
#[derive(Deserialize, DartSignal)]
pub struct SaveKeyBarLayout {
    pub rows: Vec<Vec<String>>,
}

#[derive(Serialize, RustSignal)]
pub struct KeyBarLayoutResult {
    pub ok: bool,
    pub detail: String,
}

/// 当前设置。设置变化后重发。
#[derive(Serialize, RustSignal)]
pub struct SettingsState {
    pub font_size: f64,
    pub min_font_size: f64,
    pub max_font_size: f64,
    /// 每个会话保留的滚回行数（已按本机上界收窄）。
    pub scrollback_lines: u32,
    /// 本机的滚回上界（按物理内存分档）。
    pub max_scrollback_lines: u32,
    /// 终端下方显示键位条（Esc、Tab、方向键、Ctrl、Alt…）。
    pub show_key_bar: bool,
    /// 两排按钮的稳定标识；空排允许保留。
    pub key_bar_rows: Vec<Vec<String>>,
}

/// 桌面配置协议独立于移动设置。query 忽略 base_revision；所有写操作必须携带查询所得版本。
/// operation：query/save_app/save_profile/create_profile/copy_profile/rename_profile/delete_profile。
/// save_app 使用 app；save_profile 使用 profile；创建/复制/重命名使用 name；
/// 除 query/save_app/create_profile 外均需 target_id，复制结果的 target_id 是新 ID。
#[derive(Clone, Serialize, Deserialize, DartSignal)]
pub struct DesktopSettingsRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub base_revision: u64,
    pub operation: String,
    pub target_id: String,
    pub name: String,
    pub app: Option<DesktopAppSettings>,
    pub profile: Option<DesktopTerminalProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SignalPiece)]
pub struct DesktopKeyBinding {
    /// 单个 Unicode 字符，或 Enter/Escape/Tab/Backspace/Delete/Insert/Home/End/
    /// PageUp/PageDown/ArrowUp/ArrowDown/ArrowLeft/ArrowRight/F1…F24。
    pub key: String,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SignalPiece)]
pub struct DesktopAppSettings {
    pub default_terminal_profile: String,
    /// 上游 snake_case 名称，合法值由 capabilities.color_schemes 给出。
    pub color_scheme: String,
    pub key_bindings: Vec<DesktopKeyBinding>,
}

/// 上游 16 个终端字段的无损平铺投影，version 为终端设置版本而非桌面协议版本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SignalPiece)]
pub struct DesktopTerminalProfile {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub terminal_type: String,
    pub initial_cols: u32,
    pub initial_rows: u32,
    pub scrollback_lines: u32,
    pub font_family: String,
    pub font_size: f64,
    pub color_scheme: String,
    pub key_bindings: Vec<DesktopKeyBinding>,
    pub left_alt_as_meta: bool,
    pub right_alt_as_meta: bool,
    pub enable_csi_u: bool,
    pub enable_kitty_keyboard: bool,
    pub mouse_reporting: bool,
    pub scroll_on_output: bool,
    pub scroll_on_keypress: bool,
    pub answerback: String,
}

/// 版本属于字体投影协议；无元数据和不可编辑状态只允许原样保全，不能作为新选择。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub struct ProfileFontBinding {
    pub version: u32,
    pub mode: ProfileFontMode,
    pub hash: Option<String>,
    pub uneditable_reason: Option<ProfileFontMetadataError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum ProfileFontMode {
    InheritTerminalSlot,
    ExplicitFamily,
    ExplicitCachedTtf,
    LegacyUndetermined,
    Uneditable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum ProfileFontMetadataError {
    UnsupportedVersion,
    InvalidMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum ProfileFontRuntimeSource {
    ProfileBinding,
    LegacyTerminalSlot,
    Uneditable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub enum ProfileFontFailure {
    Missing,
    Corrupt,
    Unavailable,
    InvalidResourceReference,
    UneditableMetadata,
}

/// 内容身份而非路径。family 必须是 hash 对应的 GuoshFont_<hash>。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SignalPiece)]
pub struct ProfileFontResourceRef {
    pub version: u32,
    pub hash: String,
    pub family: String,
}

/// 配置 revision 不代表偏好版本；捕获的资源不能通过重新查询当前槽加载。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, SignalPiece)]
pub struct ProfileFontSnapshot {
    pub version: u32,
    pub profile_id: String,
    pub configuration_revision: u64,
    pub saved: ProfileFontBinding,
    pub raw_family: String,
    pub runtime_source: ProfileFontRuntimeSource,
    pub captured_terminal_slot: Option<ProfileFontResourceRef>,
    pub resource: Option<ProfileFontResourceRef>,
    pub actual_family: String,
    pub fallback_reason: Option<ProfileFontFailure>,
}

/// 新字体保存协议携带明确模式；保留旧 DesktopSettingsRequest 的完整二进制布局。
/// profile 的所有字段与模式共用整个 configuration 的 base_revision CAS。
#[derive(Clone, Serialize, Deserialize, DartSignal)]
pub struct DesktopProfileSettingsRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub base_revision: u64,
    pub target_id: String,
    pub profile: DesktopTerminalProfile,
    pub font_binding: ProfileFontBinding,
}

/// profile TTF 选择不经过 FontFileRequest.apply，不改变任何全局槽。
#[derive(Clone, Serialize, Deserialize, DartSignalBinary)]
pub struct DesktopProfileTtfRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub base_revision: u64,
    pub target_id: String,
    pub profile: DesktopTerminalProfile,
}

#[derive(Clone, Serialize, Deserialize, DartSignal)]
pub struct ProfileFontResourceRequest {
    pub request_id: String,
    pub resource: ProfileFontResourceRef,
}

#[derive(Debug, Serialize, RustSignalBinary)]
pub struct ProfileFontResourceState {
    pub request_id: String,
    pub resource: Option<ProfileFontResourceRef>,
    pub family: String,
    pub failure: Option<ProfileFontFailure>,
}

#[derive(Debug, Serialize, SignalPiece)]
pub struct DesktopProfileEffective {
    pub id: String,
    /// 新会话快照的绑定：同 chord 的 profile 绑定优先于应用绑定。
    pub key_bindings: Vec<DesktopKeyBinding>,
    pub scrollback_lines: u32,
    pub scrollback_limit_reason: String,
    pub font_binding: ProfileFontBinding,
    pub font: Option<ProfileFontSnapshot>,
}

#[derive(Debug, Serialize, SignalPiece)]
pub struct DesktopConfiguration {
    pub revision: u64,
    pub app: DesktopAppSettings,
    pub profiles: Vec<DesktopTerminalProfile>,
    pub effective_profiles: Vec<DesktopProfileEffective>,
}

#[derive(Debug, Serialize, SignalPiece)]
pub struct DesktopSettingsCapabilities {
    pub available: bool,
    pub unavailable_reason: String,
    pub color_schemes: Vec<String>,
    pub key_actions: Vec<String>,
    pub min_font_size: f64,
    pub max_font_size: f64,
    pub min_scrollback_lines: u32,
    pub max_scrollback_lines: u32,
    pub min_grid_size: u32,
    pub max_grid_size: u32,
    pub profile_changes_require_new_session: bool,
}

#[derive(Debug, Serialize, SignalPiece)]
pub struct DesktopSettingsFieldError {
    pub field: String,
    pub code: String,
}

/// 只有 applied 表示持久提交成功；ok 只用于查询。错误不回显草稿、SQL 或存储内容。
/// 其余 status：validation_error/conflict/storage_error/unsupported_version/unsupported_platform。
/// conflict 的 revision 是当前版本；调用方保留草稿并显式重新查询，不自动重试覆盖。
/// snapshot 仅用于 query/applied，UI 仍须按 request_id 与 revision 丢弃过期回包。
#[derive(Debug, Serialize, RustSignal)]
pub struct DesktopSettingsResult {
    pub protocol_version: u32,
    pub request_id: String,
    pub status: String,
    pub revision: Option<u64>,
    pub target_id: String,
    pub detail: String,
    pub field_errors: Vec<DesktopSettingsFieldError>,
    pub capabilities: DesktopSettingsCapabilities,
    pub snapshot: Option<DesktopConfiguration>,
}

/// 测试走与 Dart FFI 相同的 rinf 解码和信号队列，不另建请求解析器。
#[cfg(test)]
pub(crate) fn inject_desktop_request(bytes: &[u8]) {
    DesktopSettingsRequest::send_dart_signal(bytes, &[]);
}

#[cfg(test)]
pub(crate) fn inject_profile_settings_request(bytes: &[u8]) {
    DesktopProfileSettingsRequest::send_dart_signal(bytes, &[]);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::SaveSettings;
    use rinf::DartSignal;

    #[tokio::test]
    async fn legacy_four_field_patches_use_the_real_rinf_decoder() {
        let receiver = SaveSettings::get_dart_signal_receiver();
        let patches = [
            (Some("Menlo".to_owned()), None, None, None),
            (None, Some(22.0), None, None),
            (None, None, Some(2_000u32), None),
            (None, None, None, Some(false)),
        ];
        for patch in &patches {
            let bytes = rinf::serialize(patch).expect("旧四字段补丁编码");
            SaveSettings::send_dart_signal(&bytes, &[]);
        }
        for patch in patches {
            let received = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
                .await
                .expect("补丁已解码")
                .expect("信号通道未关闭")
                .message;
            assert_eq!(
                (
                    received.font_family,
                    received.font_size,
                    received.scrollback_lines,
                    received.show_key_bar,
                ),
                patch
            );
        }
    }

    #[tokio::test]
    async fn captured_resource_and_profile_ttf_use_real_rinf_decoders_without_paths_or_slot_apply()
    {
        use super::{DesktopProfileTtfRequest, DesktopTerminalProfile, ProfileFontResourceRequest};
        use rinf::DartSignalBinary;
        let reference = crate::profile_fonts::resource_ref(&"a".repeat(64));
        let request = ProfileFontResourceRequest {
            request_id: "资源身份解码".into(),
            resource: reference.clone(),
        };
        let receiver = ProfileFontResourceRequest::get_dart_signal_receiver();
        ProfileFontResourceRequest::send_dart_signal(
            &rinf::serialize(&request).expect("资源引用编码"),
            &[],
        );
        let decoded = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .expect("资源引用解码")
            .expect("资源通道")
            .message;
        assert_eq!(decoded.request_id, request.request_id);
        assert_eq!(decoded.resource, reference);
        let profile = DesktopTerminalProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: "字体桥接 profile".into(),
            version: 1,
            terminal_type: "xterm-256color".into(),
            initial_cols: 80,
            initial_rows: 24,
            scrollback_lines: 1_000,
            font_family: "MesloLGS NF".into(),
            font_size: 14.0,
            color_scheme: "default".into(),
            key_bindings: vec![],
            left_alt_as_meta: true,
            right_alt_as_meta: true,
            enable_csi_u: false,
            enable_kitty_keyboard: false,
            mouse_reporting: true,
            scroll_on_output: true,
            scroll_on_keypress: false,
            answerback: "GuoSSHell".into(),
        };
        let request = DesktopProfileTtfRequest {
            protocol_version: 1,
            request_id: "TTF二进制解码".into(),
            base_revision: 42,
            target_id: profile.id.clone(),
            profile,
        };
        let receiver = DesktopProfileTtfRequest::get_dart_signal_receiver();
        let bytes = include_bytes!("../../../../assets/fonts/MesloLGS-NF-Regular.ttf");
        DesktopProfileTtfRequest::send_dart_signal(
            &rinf::serialize(&request).expect("TTF选择编码"),
            bytes,
        );
        let decoded = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
            .await
            .expect("TTF选择解码")
            .expect("TTF通道");
        assert_eq!(decoded.message.request_id, request.request_id);
        assert_eq!(decoded.message.profile, request.profile);
        assert_eq!(decoded.message.base_revision, 42);
        assert_eq!(decoded.binary, bytes);
    }
}
