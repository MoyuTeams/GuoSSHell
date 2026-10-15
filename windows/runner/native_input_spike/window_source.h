#ifndef GUOSSHELL_NATIVE_INPUT_SPIKE_WINDOW_SOURCE_H_
#define GUOSSHELL_NATIVE_INPUT_SPIKE_WINDOW_SOURCE_H_

#include <windows.h>

#include <array>
#include <cstddef>
#include <cstdint>

namespace guosshell::native_input_spike {

class TsfProbe;

// 仅为宿主责任接口，不是 Rust ABI。cookie 由父的同 hub bridge 绑定，
// 不能把这些数值当作可自行铸造的 Rust capability。
struct ContextIdentity {
  uint64_t owner_cookie = 0;
  uint64_t context_epoch = 0;
};

inline bool SameContext(ContextIdentity a, ContextIdentity b) noexcept {
  return a.owner_cookie == b.owner_cookie &&
         a.context_epoch == b.context_epoch;
}

enum class KeyPhase { Down, Repeat, Up };
enum class Admission { Accepted, Busy, StaleOwner, InvalidSource, Unsupported };
enum class Disposition { Consumed, Encoded, AllowNativeText, Rejected };
enum class RawProof { Unknown, KnownOutsideDomain, Trusted };
enum class RevokeAck { Confirmed, Pending, Failed };

struct NativeKeyEvent {
  ContextIdentity context;
  uint64_t native_event_id = 0;
  UINT message = 0;
  WPARAM virtual_key = 0;
  LPARAM key_bits = 0;
  DWORD message_time = 0;
  HKL layout = nullptr;
  std::array<BYTE, 256> queue_keyboard_state{};
  bool keyboard_state_available = false;
  bool local_rejected_lifecycle = false;
  KeyPhase phase = KeyPhase::Down;
  uint16_t scan_code = 0;
  bool extended = false;
  // 父注入候选意图；此处不匹配 chord、不执行菜单／壳／zoom。
  uint32_t default_candidate_intent = 0;
};

struct TextPermit {
  ContextIdentity context;
  uint64_t native_event_id = 0;
  uint64_t ticket = 0;
};

struct GateReply {
  Admission admission = Admission::Unsupported;
  Disposition disposition = Disposition::Rejected;
  RawProof raw_proof = RawProof::Unknown;
  TextPermit text;
};

// 所有函数在 source 的 STA/UI 线程同步调用，必须 nonblocking、noexcept。
// user 从绑定到 Shutdown 完成不得换成 current pane；真实 ABI 由父适配。
struct BridgeCallbacks {
  void* user = nullptr;
  Admission (*bind)(void*, HWND, ContextIdentity*) noexcept = nullptr;
  GateReply (*key)(void*, const NativeKeyEvent&) noexcept = nullptr;
  bool (*validate_text)(void*, const NativeKeyEvent&,
                        const TextPermit&) noexcept = nullptr;
  bool (*cancel_text)(void*, const TextPermit&) noexcept = nullptr;
  RevokeAck (*revoke)(void*, ContextIdentity) noexcept = nullptr;
};

enum class StopReason {
  None,
  Disabled,
  InvalidFixture,
  WrongThread,
  OsFailure,
  TsfUnavailable,
  NoFocus,
  Revoked,
  InvalidTicket,
  UnknownRaw,
  TextStoreUnsupported,
  CharacterAssociationUnsupported,
  PointerUnsupported,
  MissingPreTranslate,
  RejectionLedgerFull,
  TraceFull,
  EventIdExhausted,
  TicketCancelPending,
  RevokePending,
  RetiredReferencesPending,
  RetiredMessagesPending,
  InFlightCallPending,
};

enum class LoopRoute { ExistingRoute, DispatchOnly, Handled };
struct LoopDecision {
  LoopRoute route = LoopRoute::ExistingRoute;
  StopReason stop = StopReason::None;
  HRESULT hr = S_OK;
};

enum class TraceStage {
  GateEnter,
  GateReturn,
  TicketValidate,
  TestKeyDown,
  KeyDown,
  TestKeyUp,
  KeyUp,
  TicketCancel,
  Blocked,
  RevokeEnter,
  RevokeReturn,
};

struct TraceRecord {
  TraceStage stage;
  uint64_t event;
  LONGLONG ticks;
  HRESULT hr;
};

struct SourceOptions {
  bool opt_in = false;
  bool dedicated_fixture = false;
  BridgeCallbacks callbacks;
};

struct SourceCounters {
  uint32_t gate = 0;
  uint32_t tsf_test_down = 0;
  uint32_t tsf_down = 0;
  uint32_t tsf_test_up = 0;
  uint32_t tsf_up = 0;
  uint32_t character_messages_blocked = 0;
  uint32_t pointer_messages_blocked = 0;
  uint32_t missing_pretranslate = 0;
};

// 单次绑定、不可重新 Create／换 epoch；默认构造不注册 HWND 或 TSF。
class WindowSource final {
 public:
  static constexpr size_t kRejectionSlots = 32;
  static constexpr size_t kTraceSlots = 256;
  static constexpr size_t kRetirementDrainSlots = 64;

  WindowSource() noexcept = default;
  ~WindowSource();
  WindowSource(const WindowSource&) = delete;
  WindowSource& operator=(const WindowSource&) = delete;

  HRESULT Create(HWND run_owned_parent, HINSTANCE instance,
                 const SourceOptions& options) noexcept;
  // 必须在 GetMessage 后、任何 TSF／Translate／Flutter 分发之前调用。
  LoopDecision PreTranslate(const MSG& message,
                            uint32_t candidate_intent = 0) noexcept;
  RevokeAck BeginRevoke() noexcept;
  // 仅在父已取得同 hub 的真实撤权确认后调用，不是自行确认 Rust。
  HRESULT ConfirmRevoke() noexcept;
  HRESULT NotifyHostDeactivated() noexcept;
  // 外层入口／外调未退栈时只停 admission，返回 Pending，不授予销毁许可。
  // Pending 时保留停用的 HWND/context；父不得放行新的 editable owner。
  HRESULT Shutdown() noexcept;

  HWND hwnd() const noexcept { return hwnd_; }
  bool accepting() const noexcept { return accepting_; }
  bool core_revoked() const noexcept { return core_revoked_; }
  ContextIdentity identity() const noexcept { return identity_; }
  StopReason last_stop() const noexcept { return last_stop_; }
  const SourceCounters& counters() const noexcept { return counters_; }
  const TraceRecord* trace() const noexcept { return trace_.data(); }
  size_t trace_size() const noexcept { return trace_size_; }
  // 仅测试观察 COM 晚回调，调用者必须 Release，不能改变关联。
  HRESULT AcquireTextStoreForProbe(IUnknown** store) noexcept;
  static LONG LiveTextStores() noexcept;

 private:
  // 在 Source 存活的整个入口栈上持有；外调深度另阻止重入退休 TSF。
  class CallScope final {
   public:
    explicit CallScope(WindowSource& source, bool external = false) noexcept
        : depth_(external ? source.external_depth_ : source.entry_depth_) {
      ++depth_;
    }
    ~CallScope() { --depth_; }
    CallScope(const CallScope&) = delete;
    CallScope& operator=(const CallScope&) = delete;
   private:
    size_t& depth_;
  };

  struct RejectedKey {
    bool occupied = false;
    uint16_t scan = 0;
    bool extended = false;
  };

  static LRESULT CALLBACK WindowProc(HWND, UINT, WPARAM, LPARAM) noexcept;
  LRESULT OnMessage(UINT, WPARAM, LPARAM) noexcept;
  bool Record(TraceStage, uint64_t, HRESULT = S_OK) noexcept;
  bool OnThread() const noexcept;
  LoopDecision Block(StopReason, HRESULT = S_OK) noexcept;
  HRESULT RetireTsf() noexcept;
  HRESULT DestroyLocalResources() noexcept;

  HWND hwnd_ = nullptr;
  HINSTANCE instance_ = nullptr;
  DWORD thread_ = 0;
  ATOM window_class_ = 0;
  wchar_t class_name_[96]{};
  bool attempted_ = false;
  bool creating_ = false;
  bool stop_latched_ = false;
  bool accepting_ = false;
  bool bound_ = false;
  bool revoke_started_ = false;
  bool core_revoked_ = false;
  size_t entry_depth_ = 0;
  size_t external_depth_ = 0;
  RevokeAck revoke_ack_ = RevokeAck::Pending;
  uint64_t next_event_ = 0;
  ContextIdentity identity_;
  SourceOptions options_;
  TsfProbe* tsf_ = nullptr;
  StopReason last_stop_ = StopReason::None;
  SourceCounters counters_;
  std::array<RejectedKey, kRejectionSlots> rejected_{};
  std::array<TraceRecord, kTraceSlots> trace_{};
  size_t trace_size_ = 0;
};

}  // namespace guosshell::native_input_spike

#endif
