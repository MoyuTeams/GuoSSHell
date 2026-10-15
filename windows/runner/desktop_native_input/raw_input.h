#pragma once

#include "../native_input_surface/contracts.h"

namespace guosshell::desktop_native_input {

enum class RawStatus {
  Ready, Ignored, WrongThread, WrongFocus, StaleEpoch, ForeignOwner,
  QueryFailed, RegistrationFailed, OwnershipLost, InvalidPacket,
  InitialStateUnknown, Exhausted,
};

// 每个槽只保存本焦点域的按压前态，不保存 context、COM 或 Rust ticket。
class RawPressState final {
 public:
  void Reset(const std::array<BYTE, 256>& initial, HKL layout) noexcept;
  RawStatus Decode(HWND, const RAWKEYBOARD&, uint64_t event,
                   const guossh::native_input::ProfileIdentity&, bool altgr_layout,
                   guossh::native_input::NativeKey*) noexcept;
 private:
  enum class State : uint8_t { Up, Down, Unknown };
  std::array<State, 768> presses_{};
  std::array<BYTE, 256> physical_{};
  std::array<BYTE, 256> toggles_{};
  std::array<bool, 256> unknown_{};
};

// 应用进程的键盘注册只有一个 owner；不接管已存在的任何注册。
// HWND/epoch 由原生 pane owner 提供，不从 MethodChannel 的数值取信。
class RawInputOwner final {
 public:
  RawInputOwner() noexcept;
  ~RawInputOwner();
  RawInputOwner(const RawInputOwner&) = delete;
  RawInputOwner& operator=(const RawInputOwner&) = delete;
  RawStatus Enter(HWND, uint64_t epoch) noexcept;
  RawStatus Leave() noexcept;
  // 消费 own WM_INPUT，并在返回前调用 DefWindowProc 清理；主循环不能再 Dispatch 此包。
  RawStatus Read(const MSG&, uint64_t epoch,
                 const guossh::native_input::ProfileIdentity&, bool altgr_layout,
                 guossh::native_input::NativeKey*) noexcept;
  bool Owns(HWND hwnd) const noexcept { return hwnd_ && hwnd_ == hwnd; }
  bool Active() const noexcept { return hwnd_ != nullptr; }
  uint64_t PayloadReads() const noexcept { return reads_; }
  bool Current(const guossh::native_input::NativeKey&) const noexcept;
  bool RetirementReady(HWND) noexcept;
  uint64_t LastRegisteredEpoch() const noexcept { return greatest_epoch_; }
  static RawStatus QueryKeyboard(RAWINPUTDEVICE*, bool* present) noexcept;
 private:
  bool RegistrationMatches() const noexcept;
  const DWORD thread_;
  HWND hwnd_{};
  uint64_t epoch_{}, greatest_epoch_{}, event_{}, reads_{};
  bool entering_{}, cancelled_{};
  RawPressState presses_;
};

}  // namespace guosshell::desktop_native_input
