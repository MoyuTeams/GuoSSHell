#pragma once

#include "../native_input_spike/window_source.h"
#include "../../../native/hub/native_input_spike.h"

namespace guosshell::native_input_spike_bridge {

namespace abi = native_input_spike_abi;
namespace source = native_input_spike;

// 此标签是 caller 的证据责任，不是地址／文件名推导出的认证。
enum class TableEvidence {
  Unavailable,
  InjectedSpy,
  StandaloneCheckedFixture,
  ParentVerifiedSameSdk,
};

// 实际 host 只从父已 Verified、进程寿命 PIN 的映像 GetProcAddress 注入。
// 本模块不加载库、不拥有 HMODULE、不允许卸载；spy 不构成 actual Rust 证明。
struct FunctionsTable {
  abi::OpenFn open = nullptr;
  abi::KeyFn key = nullptr;
  abi::RevokeFn revoke = nullptr;
  abi::JoinFn join = nullptr;
  abi::InspectFn inspect = nullptr;
  abi::PeerModeFn peer_mode = nullptr;
  abi::PeerReadFn peer_read = nullptr;
  abi::SupportFn support = nullptr;
  TableEvidence evidence = TableEvidence::Unavailable;

  bool available() const noexcept;
};

// 只有固定 fixture 数字，不携带用户正文、GUI 动作对象或 action heap。
struct NumericObservation {
  std::uint64_t event_id = 0;
  std::uint64_t physical = 0;
  std::uint32_t phase = 0;
  std::uint32_t raw_source = 0;
  std::uint32_t modifiers = 0;
  abi::SpikeAdmission result{};
};

struct ObservationSink {
  void* user = nullptr;
  // 与 key 同栈，必须 noexcept、nonblocking；只观察，不执行 GUI／菜单／zoom。
  void (*observe)(void*, const NumericObservation&) noexcept = nullptr;
};

// 只能在 UI 已完成 revoke 后交给非 UI 管理 caller；不持有 Bridge 或 WindowSource。
// copied capability 的所有字段保持原样，join 不授予 DLL 卸载许可。
struct Retirement {
  abi::SpikeOwner owner{};
  abi::JoinFn join = nullptr;
  DWORD ui_thread = 0;

  std::uint32_t JoinOnManagementThread() const noexcept;
};

class NativeInputBridge final {
 public:
  static constexpr std::uint64_t kImmutableNativeDomain = 0x5741564100000000ULL;

  // Bridge、sink 与函数表映像必须存活至 source.Shutdown 退栈且全部 callback 完成。
  // source 对象不可换地址、重建或 retarget；构造本身不 open。
  NativeInputBridge(source::WindowSource& window, FunctionsTable functions,
                    bool explicit_fixture_opt_in,
                    ObservationSink sink = {}) noexcept;
  NativeInputBridge(const NativeInputBridge&) = delete;
  NativeInputBridge& operator=(const NativeInputBridge&) = delete;

  source::BridgeCallbacks callbacks() noexcept;
  bool CopyCapability(abi::SpikeOwner* out) const noexcept;
  bool ExportRetirement(Retirement* out) const noexcept;
  std::uint32_t TextSupport() const noexcept;
  std::uint32_t last_status() const noexcept { return last_status_; }
  bool revoked() const noexcept { return revoke_confirmed_; }

 private:
  static source::Admission Bind(void*, HWND, source::ContextIdentity*) noexcept;
  static source::GateReply Key(void*, const source::NativeKeyEvent&) noexcept;
  static bool ValidateText(void*, const source::NativeKeyEvent&,
                           const source::TextPermit&) noexcept;
  static bool CancelText(void*, const source::TextPermit&) noexcept;
  static source::RevokeAck Revoke(void*, source::ContextIdentity) noexcept;

  source::Admission BindWindow(HWND, source::ContextIdentity*) noexcept;
  source::GateReply AdmitKey(const source::NativeKeyEvent&) noexcept;
  source::RevokeAck RevokeContext(source::ContextIdentity) noexcept;
  bool Current(source::ContextIdentity) const noexcept;
  source::ContextIdentity identity() const noexcept;

  source::WindowSource& window_;
  const FunctionsTable functions_;
  const bool opt_in_;
  const ObservationSink sink_;
  abi::SpikeOwner owner_{};
  // 只锁存来源／准入拒绝，不匹配绑定、不分配 seq；原 physical Up 清除。
  std::array<std::uint64_t, source::WindowSource::kRejectionSlots> rejected_{};
  DWORD ui_thread_ = 0;
  bool attempted_ = false;
  bool bound_ = false;
  bool revoke_started_ = false;
  bool revoke_confirmed_ = false;
  std::uint32_t last_status_ = abi::kDisabled;
};

}  // namespace guosshell::native_input_spike_bridge
