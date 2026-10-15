#include "native_input_spike_bridge.h"

#include <algorithm>

namespace guosshell::native_input_spike_bridge {
namespace {

source::Admission MapStatus(std::uint32_t status) noexcept {
  switch (status) {
    case abi::kOk: return source::Admission::Accepted;
    case abi::kBusy: return source::Admission::Busy;
    case abi::kStaleOwner: return source::Admission::StaleOwner;
    case abi::kInvalid:
    case abi::kInvalidSource: return source::Admission::InvalidSource;
    default: return source::Admission::Unsupported;
  }
}

bool Phase(const source::NativeKeyEvent& event, std::uint32_t* out) noexcept {
  switch (event.phase) {
    case source::KeyPhase::Down: *out = abi::kDown; return true;
    case source::KeyPhase::Repeat: *out = abi::kRepeat; return true;
    case source::KeyPhase::Up: *out = abi::kUp; return true;
    default: return false;
  }
}

bool SameMessageFacts(const source::NativeKeyEvent& event,
                      std::uint32_t phase) noexcept {
  const bool down = event.message == WM_KEYDOWN || event.message == WM_SYSKEYDOWN;
  const bool up = event.message == WM_KEYUP || event.message == WM_SYSKEYUP;
  const auto bits = static_cast<std::uint64_t>(event.key_bits);
  return (down || up) && event.scan_code == ((bits >> 16) & 0xff) &&
         event.extended == ((bits & (std::uint64_t{1} << 24)) != 0) &&
         up == (phase == abi::kUp) &&
         up == ((bits & (std::uint64_t{1} << 31)) != 0) &&
         (up || (phase == abi::kRepeat) ==
                    ((bits & (std::uint64_t{1} << 30)) != 0));
}

bool NamedCode(WPARAM vkey, std::uint32_t* kind,
               std::uint32_t* value) noexcept {
  *value = 0;
  if (vkey >= VK_F1 && vkey <= VK_F24) {
    *kind = abi::kFunction;
    *value = static_cast<std::uint32_t>(vkey - VK_F1 + 1);
    return true;
  }
  switch (vkey) {
    case VK_RETURN: *kind = abi::kEnter; break;
    case VK_ESCAPE: *kind = abi::kEscape; break;
    case VK_TAB: *kind = abi::kTab; break;
    case VK_BACK: *kind = abi::kBackspace; break;
    case VK_DELETE: *kind = abi::kDelete; break;
    case VK_INSERT: *kind = abi::kInsert; break;
    case VK_HOME: *kind = abi::kHome; break;
    case VK_END: *kind = abi::kEnd; break;
    case VK_PRIOR: *kind = abi::kPageUp; break;
    case VK_NEXT: *kind = abi::kPageDown; break;
    case VK_UP: *kind = abi::kArrowUp; break;
    case VK_DOWN: *kind = abi::kArrowDown; break;
    case VK_LEFT: *kind = abi::kArrowLeft; break;
    case VK_RIGHT: *kind = abi::kArrowRight; break;
    default: return false;
  }
  return true;
}

bool Modifiers(const source::NativeKeyEvent& event, abi::SpikeKey* key) noexcept {
  if (!event.keyboard_state_available) return false;
  const auto down = [&event](unsigned vkey) noexcept {
    return (event.queue_keyboard_state[vkey] & 0x80) != 0;
  };
  // aggregate 与左右键必须一致；RAlt 无法在本波排除 AltGr／synthetic Ctrl。
  if (down(VK_RMENU) ||
      down(VK_SHIFT) != (down(VK_LSHIFT) || down(VK_RSHIFT)) ||
      down(VK_CONTROL) != (down(VK_LCONTROL) || down(VK_RCONTROL)) ||
      down(VK_MENU) != down(VK_LMENU)) return false;
  key->modifiers = (down(VK_SHIFT) ? abi::kShift : 0U) |
                   (down(VK_CONTROL) ? abi::kControl : 0U) |
                   (down(VK_MENU) ? abi::kAlt : 0U) |
                   (down(VK_LWIN) || down(VK_RWIN) ? abi::kSuper : 0U);
  key->alt_source = 1;
  key->alt_sides = down(VK_LMENU) ? 1U : 0U;
  return true;
}

bool ValidResult(std::uint32_t returned, const abi::SpikeAdmission& result) noexcept {
  if (returned > abi::kActiveKeyBudget || result.version != abi::kVersion ||
      result.size != sizeof(result) || result.status != returned ||
      result.action > 16 || result.byte_len > 32) return false;
  if (returned != abi::kOk) {
    // Unsupported 的实际 policy 结果可以已有 seq，但不能含已放行字节／动作。
    return result.disposition == 0 && result.action == 0 && result.byte_len == 0;
  }
  if (!result.seq) return false;
  switch (result.disposition) {
    case 0:
    case 2: return result.action == 0 && result.byte_len == 0;
    case 1: return result.action == 0;
    case 4: return result.action != 0 && result.byte_len == 0;
    // 此波不存在 NativeText／paste ticket，不能将任何结果变为 AllowNativeText。
    default: return false;
  }
}

}  // namespace

bool FunctionsTable::available() const noexcept {
  const bool evidence_ok = evidence == TableEvidence::InjectedSpy ||
      evidence == TableEvidence::StandaloneCheckedFixture ||
      evidence == TableEvidence::ParentVerifiedSameSdk;
  return evidence_ok && open && key && revoke && join && inspect && peer_mode &&
         peer_read && support && command && copy && projection;
}

std::uint32_t Retirement::JoinOnManagementThread() const noexcept {
  if (!join || !owner.handle || !owner.native_context || !ui_thread ||
      GetCurrentThreadId() == ui_thread) return abi::kInvalidSource;
  return join(&owner);
}

NativeInputBridge::NativeInputBridge(source::WindowSource& window,
    FunctionsTable functions, bool opt_in, ObservationSink sink) noexcept
    : window_(window), functions_(functions), opt_in_(opt_in), sink_(sink) {}

source::BridgeCallbacks NativeInputBridge::callbacks() noexcept {
  return {this, Bind, Key, ValidateText, CancelText, Revoke};
}

source::ContextIdentity NativeInputBridge::identity() const noexcept {
  return {owner_.handle, owner_.generation};
}

bool NativeInputBridge::Current(source::ContextIdentity context) const noexcept {
  return bound_ && GetCurrentThreadId() == ui_thread_ &&
         source::SameContext(context, identity());
}

bool NativeInputBridge::CopyCapability(abi::SpikeOwner* out) const noexcept {
  if (!out || !bound_ || GetCurrentThreadId() != ui_thread_) return false;
  *out = owner_;
  return true;
}

bool NativeInputBridge::ExportRetirement(Retirement* out) const noexcept {
  if (!out || !revoke_confirmed_ || GetCurrentThreadId() != ui_thread_) return false;
  *out = {owner_, functions_.join, ui_thread_};
  return true;
}

std::uint32_t NativeInputBridge::TextSupport() const noexcept {
  if (!bound_ || !functions_.available() || !opt_in_) return abi::kDisabled;
  if (GetCurrentThreadId() != ui_thread_) return abi::kInvalidSource;
  return functions_.support(&owner_, 2);
}

source::Admission NativeInputBridge::BindWindow(HWND hwnd,
    source::ContextIdentity* out) noexcept {
  if (out) *out = {};
  if (attempted_) return source::Admission::StaleOwner;
  attempted_ = true;
  if (!opt_in_ || !functions_.available()) return source::Admission::Unsupported;
  DWORD process = 0;
  const DWORD thread = GetWindowThreadProcessId(hwnd, &process);
  // WindowSource 已建立实际 HWND；对象地址不是 HWND 数字或 Dart token。
  if (!out || !hwnd || hwnd != window_.hwnd() || !IsWindow(hwnd) ||
      process != GetCurrentProcessId() || thread != GetCurrentThreadId() ||
      GetWindowLongPtrW(hwnd, GWLP_USERDATA) != reinterpret_cast<LONG_PTR>(&window_)) {
    last_status_ = abi::kInvalidSource;
    return source::Admission::InvalidSource;
  }
  const abi::SpikeOpen request{abi::kVersion, sizeof(abi::SpikeOpen), 1, 0,
      &window_, reinterpret_cast<const std::uint8_t*>(abi::kFixtureName),
      sizeof(abi::kFixtureName) - 1, abi::kDeclaredSynthetic, abi::kFixturePlatform,
      reinterpret_cast<std::uintptr_t>(&window_)};
  abi::SpikeOwner result{};
  last_status_ = functions_.open(&request, &result);
  if (last_status_ != abi::kOk) return MapStatus(last_status_);
  // 全部工厂字段保存；不按 current pane 重构 session／generation／identity。
  owner_ = result;
  ui_thread_ = thread;
  bound_ = true;
  if (!result.handle || result.native_context != &window_ || !result.session ||
      result.reserved || !result.generation ||
      std::all_of(std::begin(result.identity), std::end(result.identity),
                  [](auto value) { return value == 0; })) {
    revoke_started_ = true;
    revoke_confirmed_ = functions_.revoke(&owner_) == abi::kOk;
    last_status_ = abi::kInvalidSource;
    return source::Admission::InvalidSource;
  }
  *out = identity();
  return source::Admission::Accepted;
}

source::GateReply NativeInputBridge::AdmitKey(
    const source::NativeKeyEvent& event) noexcept {
  source::GateReply reply;
  if (!Current(event.context)) {
    reply.admission = source::Admission::InvalidSource;
    last_status_ = abi::kInvalidSource;
    return reply;
  }
  if (revoke_started_) {
    reply.admission = source::Admission::StaleOwner;
    last_status_ = abi::kStaleOwner;
    return reply;
  }
  abi::SpikeKey key{};
  key.version = abi::kVersion;
  key.size = sizeof(key);
  key.native_event = event.native_event_id;
  // 固定 fixture 协议 revision；不是把 HKL／PostMessage 签成生产来源。
  key.layout_revision = 1;
  key.same_event_state = static_cast<std::uint64_t>(event.key_bits);
  // 缺 scan 无法延续同一 physical；停 owner，不伪造 NonZero 或给 Rust 坏枚举。
  if (!event.scan_code || event.scan_code > 0xff || !Phase(event, &key.phase)) {
    RevokeContext(event.context);
    last_status_ = abi::kInvalid;
    reply.admission = source::Admission::InvalidSource;
    return reply;
  }
  key.physical = kImmutableNativeDomain | event.scan_code |
                 (event.extended ? 0x100ULL : 0ULL);
  auto latched = std::find(rejected_.begin(), rejected_.end(), key.physical);
  const bool candidate_valid = event.default_candidate_intent <= 16;
  key.default_action = candidate_valid ? event.default_candidate_intent : 0;
  std::uint32_t kind = 0;
  std::uint32_t value = 0;
  const bool trusted = latched == rejected_.end() &&
      !event.local_rejected_lifecycle && candidate_valid &&
      SameMessageFacts(event, key.phase) && NamedCode(event.virtual_key, &kind, &value) &&
      Modifiers(event, &key);
  if (trusted) {
    key.raw_source = abi::kRawVerified;
    key.raw_kind = key.code_kind = key.legacy_kind = kind;
    key.raw_value = key.code_value = key.legacy_value = value;
    key.dual_source = 1;
    key.text_ownership = abi::kTerminalKey;
    reply.raw_proof = source::RawProof::Trusted;
  }
  // Unknown 仍调用 Rust typed reject；Rust policy／UI ledger 延续到原 physical Up。
  abi::SpikeAdmission result{};
  const auto status = functions_.key(&owner_, &key, &result);
  last_status_ = status;
  // FFI 的 capability lookup 失败发生在写 out 之前；按真实返回值拒绝。
  if ((status == abi::kStaleOwner || status == abi::kInvalidSource) &&
      result.version == 0 && result.size == 0) {
    reply.admission = MapStatus(status);
    return reply;
  }
  if (!ValidResult(status, result) ||
      (result.action && (key.phase != abi::kDown || !trusted)) ||
      (!trusted && status == abi::kOk && result.disposition != 0 &&
       result.disposition != 2)) {
    RevokeContext(event.context);
    last_status_ = abi::kInvalid;
    reply.admission = source::Admission::Unsupported;
    return reply;
  }
  if (key.phase == abi::kUp) {
    if (latched != rejected_.end()) *latched = 0;
  } else if (!trusted || status != abi::kOk || result.disposition == 0) {
    if (latched == rejected_.end()) latched = std::find(rejected_.begin(), rejected_.end(), 0);
    if (latched == rejected_.end()) {
      RevokeContext(event.context);
      last_status_ = abi::kExhausted;
      reply.admission = source::Admission::Unsupported;
      return reply;
    }
    *latched = key.physical;
  }
  reply.admission = MapStatus(result.status);
  if (status == abi::kOk) {
    switch (result.disposition) {
      case 1: reply.disposition = source::Disposition::Encoded; break;
      case 2:
      case 4: reply.disposition = source::Disposition::Consumed; break;
      default: break;
    }
  }
  if (sink_.observe) {
    const NumericObservation observation{event.native_event_id, key.physical,
        key.phase, key.raw_source, key.modifiers, result};
    sink_.observe(sink_.user, observation);
  }
  return reply;
}

source::RevokeAck NativeInputBridge::RevokeContext(
    source::ContextIdentity context) noexcept {
  if (!Current(context)) return source::RevokeAck::Failed;
  revoke_started_ = true;
  if (revoke_confirmed_) return source::RevokeAck::Confirmed;
  last_status_ = functions_.revoke(&owner_);
  revoke_confirmed_ = last_status_ == abi::kOk;
  if (revoke_confirmed_) return source::RevokeAck::Confirmed;
  return last_status_ == abi::kBusy ? source::RevokeAck::Pending
                                   : source::RevokeAck::Failed;
}

source::Admission NativeInputBridge::Bind(void* user, HWND hwnd,
    source::ContextIdentity* out) noexcept {
  return static_cast<NativeInputBridge*>(user)->BindWindow(hwnd, out);
}

source::GateReply NativeInputBridge::Key(void* user,
    const source::NativeKeyEvent& event) noexcept {
  return static_cast<NativeInputBridge*>(user)->AdmitKey(event);
}

bool NativeInputBridge::ValidateText(void* user, const source::NativeKeyEvent&,
    const source::TextPermit&) noexcept {
  auto& bridge = *static_cast<NativeInputBridge*>(user);
  bridge.last_status_ = bridge.TextSupport();
  return false;  // 尚未接入真实 text ticket；support 的 OK 不授予 native-text 许可。
}

bool NativeInputBridge::CancelText(void* user, const source::TextPermit&) noexcept {
  auto& bridge = *static_cast<NativeInputBridge*>(user);
  bridge.last_status_ = bridge.TextSupport();
  return false;
}

source::RevokeAck NativeInputBridge::Revoke(void* user,
    source::ContextIdentity context) noexcept {
  return static_cast<NativeInputBridge*>(user)->RevokeContext(context);
}

}  // namespace guosshell::native_input_spike_bridge
