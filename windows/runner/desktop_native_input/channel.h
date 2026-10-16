#ifndef RUNNER_DESKTOP_NATIVE_INPUT_CHANNEL_H_
#define RUNNER_DESKTOP_NATIVE_INPUT_CHANNEL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>

#include <memory>
#include <list>

#include "runtime_module.h"
#include "host.h"

namespace guosshell::desktop_native_input {

class Channel {
 public:
  explicit Channel(flutter::BinaryMessenger* messenger, HWND view = nullptr);
  ~Channel();
  Channel(const Channel&) = delete;
  Channel& operator=(const Channel&) = delete;
  bool BeforeTranslate(const MSG&);
  void InvalidateGeometry();
  void Deactivate();
  bool PrepareClose();
  static bool RetainedAtExit() noexcept { return retained_at_exit_; }

 private:
  friend struct ChannelTestAccess;
  struct PendingCall {
    std::string method;
    Map arguments;
    std::unique_ptr<flutter::MethodResult<Value>> result;
    ULONGLONG deadline{}, next{};
    size_t bytes{};
  };
  void PollPending();
  void HandleMethodCall(
      const flutter::MethodCall<flutter::EncodableValue>& call,
      std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result);

  RuntimeModule runtime_;
  const HWND view_;
  std::unique_ptr<Host> host_;
  static bool retained_at_exit_;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  std::list<PendingCall> pending_;
  size_t pending_bytes_{};
  bool polling_{};
};

}  // namespace guosshell::desktop_native_input

#endif  // RUNNER_DESKTOP_NATIVE_INPUT_CHANNEL_H_
