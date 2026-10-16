#pragma once
#include "codec.h"
#include <functional>
#include <memory>

namespace guosshell::desktop_native_input {
class Host final {
 public:
  using Notify = std::function<void(const char*, Map)>;
  Host(HWND view, const NormalApi&, Notify);
  ~Host();
  Map Call(const std::string& method, const Map&);
  bool BeforeTranslate(const MSG&);
  void Pump();
  void InvalidateAll();
  void Deactivate();
  bool Close();
  bool Closed() const;
  void DetachChannel();
  void SetPollCallback(std::function<void()>);
  void FailPane(const std::string&, const char*);
 private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};
}  // namespace guosshell::desktop_native_input
