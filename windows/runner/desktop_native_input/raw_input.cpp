#include "raw_input.h"

#include <atomic>

namespace guosshell::desktop_native_input {
namespace {
std::atomic<RawInputOwner*> process_owner{};
constexpr USHORT kGenericDesktop = 1;
constexpr USHORT kKeyboard = 6;
bool OwnFocus(HWND hwnd) noexcept {
  return GetFocus() == hwnd && GetForegroundWindow() == GetAncestor(hwnd, GA_ROOT);
}

UINT PhysicalVk(const RAWKEYBOARD& packet, HKL layout) noexcept {
  if (packet.VKey == VK_SHIFT)
    return MapVirtualKeyExW(packet.MakeCode, MAPVK_VSC_TO_VK_EX, layout);
  if (packet.VKey == VK_CONTROL) return packet.Flags & RI_KEY_E0 ? VK_RCONTROL : VK_LCONTROL;
  if (packet.VKey == VK_MENU) return packet.Flags & RI_KEY_E0 ? VK_RMENU : VK_LMENU;
  return packet.VKey;
}
void Aggregate(std::array<BYTE, 256>& state) noexcept {
  state[VK_SHIFT] = state[VK_LSHIFT] | state[VK_RSHIFT];
  state[VK_CONTROL] = state[VK_LCONTROL] | state[VK_RCONTROL];
  state[VK_MENU] = state[VK_LMENU] | state[VK_RMENU];
}
UINT MessageVk(UINT vk) noexcept {
  if (vk == VK_LSHIFT || vk == VK_RSHIFT) return VK_SHIFT;
  if (vk == VK_LCONTROL || vk == VK_RCONTROL) return VK_CONTROL;
  if (vk == VK_LMENU || vk == VK_RMENU) return VK_MENU;
  return vk;
}
bool DrainOwnedRaw(HWND hwnd) noexcept {
  MSG message{};
  for (unsigned i = 0; i < 256; ++i) {
    if (!PeekMessageW(&message, hwnd, WM_INPUT, WM_INPUT, PM_REMOVE)) return true;
    // 旧焦点包只做系统清理，不查询 header 或 payload。
    DefWindowProcW(hwnd, message.message, message.wParam, message.lParam);
  }
  return PeekMessageW(&message, hwnd, WM_INPUT, WM_INPUT, PM_NOREMOVE) == FALSE;
}
}

void RawPressState::Reset(const std::array<BYTE, 256>& initial, HKL layout) noexcept {
  presses_.fill(State::Up);
  physical_.fill(0);
  toggles_.fill(0);
  for (size_t vk = 0; vk < initial.size(); ++vk) {
    toggles_[vk] = initial[vk] & 1;
    unknown_[vk] = (initial[vk] & 0x80) != 0;
  }
  unknown_[VK_SHIFT] = unknown_[VK_SHIFT] || unknown_[VK_LSHIFT] || unknown_[VK_RSHIFT];
  unknown_[VK_CONTROL] = unknown_[VK_CONTROL] || unknown_[VK_LCONTROL] || unknown_[VK_RCONTROL];
  unknown_[VK_MENU] = unknown_[VK_MENU] || unknown_[VK_LMENU] || unknown_[VK_RMENU];
  for (size_t slot = 0; slot < presses_.size(); ++slot) {
    const UINT scan = static_cast<UINT>(slot & 255) |
        (slot >= 512 ? 0xe100u : slot >= 256 ? 0xe000u : 0u);
    const UINT vk = MapVirtualKeyExW(scan, MAPVK_VSC_TO_VK_EX, layout);
    if (vk < initial.size() && (initial[vk] & 0x80)) presses_[slot] = State::Unknown;
  }
}

RawStatus RawPressState::Decode(HWND hwnd, const RAWKEYBOARD& packet, uint64_t event,
    const guossh::native_input::ProfileIdentity& profile, bool altgr_layout,
    guossh::native_input::NativeKey* output) noexcept {
  if (!output || !event || !profile.hkl || !packet.MakeCode || packet.MakeCode > 255 ||
      packet.VKey == 255 || packet.VKey >= 256 || packet.VKey == VK_PACKET ||
      packet.VKey == VK_PROCESSKEY ||
      (packet.Flags & ~(RI_KEY_BREAK | RI_KEY_E0 | RI_KEY_E1)) ||
      ((packet.Flags & RI_KEY_E0) && (packet.Flags & RI_KEY_E1))) return RawStatus::InvalidPacket;
  const bool up = (packet.Flags & RI_KEY_BREAK) != 0;
  if (packet.Message != static_cast<UINT>(up ? WM_KEYUP : WM_KEYDOWN) &&
      packet.Message != static_cast<UINT>(up ? WM_SYSKEYUP : WM_SYSKEYDOWN)) return RawStatus::InvalidPacket;
  const UINT vk = PhysicalVk(packet, profile.hkl);
  if (!vk || vk >= 256) return RawStatus::InvalidPacket;
  const size_t slot = packet.MakeCode + (packet.Flags & RI_KEY_E1 ? 512 : packet.Flags & RI_KEY_E0 ? 256 : 0);
  auto& press = presses_[slot];
  if (press == State::Unknown) {
    // 缺少按压起点不签发新的 Down/Repeat；不得把未知状态解释为已证明。
    if (up) {
      press = State::Up;
      unknown_[vk] = false;
      unknown_[VK_SHIFT] = unknown_[VK_LSHIFT] || unknown_[VK_RSHIFT];
      unknown_[VK_CONTROL] = unknown_[VK_LCONTROL] || unknown_[VK_RCONTROL];
      unknown_[VK_MENU] = unknown_[VK_LMENU] || unknown_[VK_RMENU];
    }
    return RawStatus::InitialStateUnknown;
  }
  if (unknown_[VK_SHIFT] || unknown_[VK_CONTROL] || unknown_[VK_MENU] ||
      unknown_[VK_LWIN] || unknown_[VK_RWIN]) return RawStatus::InitialStateUnknown;
  if (up && press != State::Down) return RawStatus::InitialStateUnknown;
  const bool previous = press == State::Down;
  const bool alt_before = physical_[VK_MENU] != 0;
  press = up ? State::Up : State::Down;
  physical_[vk] = up ? 0 : 0x80;
  Aggregate(physical_);
  if (!up && !previous && (vk == VK_CAPITAL || vk == VK_NUMLOCK || vk == VK_SCROLL)) toggles_[vk] ^= 1;

  guossh::native_input::NativeKey key{};
  key.event_id = event;
  key.layout = profile;
  key.scan = packet.MakeCode;
  key.extended = (packet.Flags & RI_KEY_E0) != 0;
  key.e1 = (packet.Flags & RI_KEY_E1) != 0;
  key.physical = slot + 1;
  key.repeat = previous && !up;
  key.up = up;
  key.canonical_raw = true;
  key.physical_state = physical_;
  key.queue_state = physical_;
  for (size_t i = 0; i < key.queue_state.size(); ++i) key.queue_state[i] |= toggles_[i];
  // 只改 TSF 的兼容键态；从不写回 physical_ 或制造一个 Ctrl press。
  if (altgr_layout && physical_[VK_RMENU]) key.queue_state[VK_CONTROL] |= 0x80;
  const UINT message_vk = MessageVk(vk);
  const bool system = alt_before || (!up && message_vk == VK_MENU) || vk == VK_F10;
  key.message.hwnd = hwnd;
  key.message.message = system ? (up ? WM_SYSKEYUP : WM_SYSKEYDOWN) : (up ? WM_KEYUP : WM_KEYDOWN);
  key.message.wParam = message_vk;
  const uint32_t bits = 1u | (static_cast<uint32_t>(packet.MakeCode) << 16) |
      (key.extended ? 1u << 24 : 0) | (alt_before ? 1u << 29 : 0) |
      (previous ? 1u << 30 : 0) | (up ? 1u << 31 : 0);
  key.message.lParam = static_cast<LPARAM>(bits);
  *output = key;
  return RawStatus::Ready;
}

RawInputOwner::RawInputOwner() noexcept : thread_(GetCurrentThreadId()) {}
RawInputOwner::~RawInputOwner() { Leave(); }

RawStatus RawInputOwner::QueryKeyboard(RAWINPUTDEVICE* output, bool* present) noexcept {
  if (!output || !present) return RawStatus::QueryFailed;
  *present = false;
  std::array<RAWINPUTDEVICE, 128> devices{};
  UINT count = static_cast<UINT>(devices.size());
  const UINT received = GetRegisteredRawInputDevices(devices.data(), &count, sizeof(RAWINPUTDEVICE));
  if (received == UINT_MAX || received > devices.size()) return RawStatus::QueryFailed;
  for (UINT i = 0; i < received; ++i) {
    if (devices[i].usUsagePage != kGenericDesktop || devices[i].usUsage != kKeyboard) continue;
    if (*present) return RawStatus::QueryFailed;
    *output = devices[i];
    *present = true;
  }
  return RawStatus::Ready;
}
bool RawInputOwner::RegistrationMatches() const noexcept {
  RAWINPUTDEVICE device{};
  bool present{};
  return QueryKeyboard(&device, &present) == RawStatus::Ready && present &&
      device.hwndTarget == hwnd_ && device.dwFlags == RIDEV_NOLEGACY;
}
RawStatus RawInputOwner::Enter(HWND hwnd, uint64_t epoch) noexcept {
  if (GetCurrentThreadId() != thread_) return RawStatus::WrongThread;
  if (entering_) return RawStatus::ForeignOwner;
  if (!epoch || epoch <= greatest_epoch_) return RawStatus::StaleEpoch;
  DWORD process{};
  if (!IsWindow(hwnd) || GetWindowThreadProcessId(hwnd, &process) != thread_ ||
      process != GetCurrentProcessId() || !OwnFocus(hwnd)) return RawStatus::WrongFocus;
  if (hwnd_) return RawStatus::ForeignOwner;
  RawInputOwner* expected = nullptr;
  if (!process_owner.compare_exchange_strong(expected, this)) return RawStatus::ForeignOwner;
  struct Entering {
    bool& flag;
    explicit Entering(bool& value) noexcept : flag(value) { flag = true; }
    ~Entering() { flag = false; }
  } entering(entering_);
  cancelled_ = false;
  RAWINPUTDEVICE current{};
  bool present{};
  RawStatus status = QueryKeyboard(&current, &present);
  if (status != RawStatus::Ready || present) {
    process_owner.store(nullptr);
    return present ? RawStatus::ForeignOwner : status;
  }
  if (!DrainOwnedRaw(hwnd)) { process_owner.store(nullptr); return RawStatus::Exhausted; }
  if (cancelled_ || !OwnFocus(hwnd)) { process_owner.store(nullptr); return RawStatus::WrongFocus; }
  std::array<BYTE, 256> initial{};
  if (!GetKeyboardState(initial.data())) { process_owner.store(nullptr); return RawStatus::QueryFailed; }
  status = QueryKeyboard(&current, &present);
  if (status != RawStatus::Ready || present) {
    process_owner.store(nullptr);
    return present ? RawStatus::ForeignOwner : status;
  }
  RAWINPUTDEVICE device{kGenericDesktop, kKeyboard, RIDEV_NOLEGACY, hwnd};
  if (!RegisterRawInputDevices(&device, 1, sizeof(device))) {
    process_owner.store(nullptr);
    return RawStatus::RegistrationFailed;
  }
  hwnd_ = hwnd;
  epoch_ = greatest_epoch_ = epoch;
  // 只在自身前台焦点域取一次高位快照，不读历史低位；已有按住状态只能增加 Unknown。
  // NOLEGACY 下线程队列态可能滞后，不能据其低位把域外已按住的键视为新 Down。
  if (!OwnFocus(hwnd_)) { Leave(); return RawStatus::WrongFocus; }
  for (int vk = 1; vk < 256; ++vk) {
    if (!OwnFocus(hwnd_)) { Leave(); return RawStatus::WrongFocus; }
    if (GetAsyncKeyState(vk) & 0x8000) initial[static_cast<size_t>(vk)] |= 0x80;
  }
  presses_.Reset(initial, GetKeyboardLayout(thread_));
  if (!RegistrationMatches()) { Leave(); return RawStatus::OwnershipLost; }
  if (cancelled_ || !OwnFocus(hwnd_)) { Leave(); return RawStatus::WrongFocus; }
  return RawStatus::Ready;
}
RawStatus RawInputOwner::Leave() noexcept {
  if (GetCurrentThreadId() != thread_) return RawStatus::WrongThread;
  if (entering_) cancelled_ = true;
  if (!hwnd_) return RawStatus::Ready;
  RawStatus status = RawStatus::OwnershipLost;
  if (RegistrationMatches()) {
    RAWINPUTDEVICE device{kGenericDesktop, kKeyboard, RIDEV_REMOVE, nullptr};
    if (!RegisterRawInputDevices(&device, 1, sizeof(device))) return RawStatus::RegistrationFailed;
    RAWINPUTDEVICE current{};
    bool present{};
    status = QueryKeyboard(&current, &present);
    if (status == RawStatus::Ready && present) status = RawStatus::OwnershipLost;
    if (status == RawStatus::Ready && !DrainOwnedRaw(hwnd_)) status = RawStatus::Exhausted;
  }
  hwnd_ = nullptr;
  epoch_ = 0;
  RawInputOwner* expected = this;
  process_owner.compare_exchange_strong(expected, nullptr);
  return status;
}
bool RawInputOwner::Current(const guossh::native_input::NativeKey& key) const noexcept {
  return GetCurrentThreadId() == thread_ && hwnd_ && key.canonical_raw &&
      key.source_epoch == epoch_ && key.message.hwnd == hwnd_ && OwnFocus(hwnd_) &&
      key.layout.hkl == GetKeyboardLayout(thread_) && RegistrationMatches();
}
bool RawInputOwner::RetirementReady(HWND retired) noexcept {
  if (GetCurrentThreadId() != thread_ || hwnd_) return false;
  RAWINPUTDEVICE current{}; bool present{};
  return QueryKeyboard(&current, &present) == RawStatus::Ready && !present && DrainOwnedRaw(retired);
}
RawStatus RawInputOwner::Read(const MSG& message, uint64_t epoch,
    const guossh::native_input::ProfileIdentity& profile, bool altgr_layout,
    guossh::native_input::NativeKey* output) noexcept {
  if (GetCurrentThreadId() != thread_) return RawStatus::WrongThread;
  if (message.message != WM_INPUT || !hwnd_ || message.hwnd != hwnd_) return RawStatus::Ignored;
  struct Cleanup {
    const MSG& message;
    ~Cleanup() { DefWindowProcW(message.hwnd, message.message, message.wParam, message.lParam); }
  } cleanup{message};
  if (!epoch || epoch != epoch_) return RawStatus::StaleEpoch;
  if (!OwnFocus(hwnd_) || GetKeyboardLayout(thread_) != profile.hkl) return RawStatus::WrongFocus;
  if (!RegistrationMatches()) return RawStatus::OwnershipLost;
  if (GET_RAWINPUT_CODE_WPARAM(message.wParam) != RIM_INPUT) return RawStatus::InvalidPacket;
  if (event_ == UINT64_MAX) return RawStatus::Exhausted;
  RAWINPUT packet{};
  UINT size = sizeof(packet);
  ++reads_;
  const UINT received = GetRawInputData(reinterpret_cast<HRAWINPUT>(message.lParam), RID_INPUT,
      &packet, &size, sizeof(RAWINPUTHEADER));
  if (received == UINT_MAX || received != sizeof(RAWINPUTHEADER) + sizeof(RAWKEYBOARD) ||
      packet.header.dwSize != received || packet.header.dwType != RIM_TYPEKEYBOARD ||
      !packet.header.hDevice || packet.header.wParam != RIM_INPUT) return RawStatus::InvalidPacket;
  if (!OwnFocus(hwnd_) || epoch != epoch_) return RawStatus::WrongFocus;
  const RawStatus status = presses_.Decode(hwnd_, packet.data.keyboard, ++event_, profile, altgr_layout, output);
  if (status == RawStatus::Ready) output->source_epoch = epoch;
  return status;
}

}  // namespace guosshell::desktop_native_input
