#ifndef GUOSSHELL_NATIVE_INPUT_SPIKE_TSF_PROBE_H_
#define GUOSSHELL_NATIVE_INPUT_SPIKE_TSF_PROBE_H_

#include <msctf.h>
#include <textstor.h>

#include "window_source.h"

namespace guosshell::native_input_spike {

class UnsupportedTextStore;

// 真实 TSF 注册／keystroke 调用，明确不提供可编辑文本能力。
class TsfProbe final {
 public:
  static constexpr LONG kContextCapacity = 4;
  TsfProbe() noexcept = default;
  ~TsfProbe();
  TsfProbe(const TsfProbe&) = delete;
  TsfProbe& operator=(const TsfProbe&) = delete;

  HRESULT Initialize(HWND, ContextIdentity) noexcept;
  // 仅合成 COM case 使用同一 ACP 实现，不注册系统 TSF，不提供编辑能力。
  HRESULT InitializeStoreForProbe(HWND, ContextIdentity) noexcept;
  HRESULT Focus() noexcept;
  HRESULT TestKey(const MSG&, bool up, BOOL* eaten) noexcept;
  HRESULT Key(const MSG&, bool up, BOOL* eaten) noexcept;
  void StopAdmissions() noexcept;
  HRESULT Retire() noexcept;
  bool CanRelease() const noexcept;
  HRESULT AcquireStore(IUnknown**) noexcept;
  static LONG LiveStores() noexcept;

 private:
  void ReleaseSystemObjects() noexcept;
  ITfThreadMgr* thread_mgr_ = nullptr;
  ITfKeystrokeMgr* keystroke_mgr_ = nullptr;
  ITfDocumentMgr* document_ = nullptr;
  ITfContext* context_ = nullptr;
  UnsupportedTextStore* store_ = nullptr;
  TfClientId client_ = TF_CLIENTID_NULL;
  bool activated_ = false;
  bool pushed_ = false;
  bool retired_ = false;
};

}  // namespace guosshell::native_input_spike

#endif
