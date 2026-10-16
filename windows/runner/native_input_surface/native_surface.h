#pragma once

#include "layout_metadata.h"
#include "text_store.h"

namespace guossh::native_input {

struct DispatchResult {
  bool owned{};
  Disposition disposition{Disposition::Rejected};
  TextRoute route{TextRoute::None};
  Fault fault{Fault::None};
  HRESULT native_result{S_OK};
  uint32_t tsf_calls{}, translates{}, derived{};
};

class NativeSurface final : public ITfInputProcessorProfileActivationSink {
 public:
  static NativeSurface* Create(HWND, OwnerRef, std::shared_ptr<Callbacks>, std::shared_ptr<NativePermit>, NativeOrigin) noexcept;
  // manager 必须是调用线程已激活的应用 manager；此处不建线程或第二 hub。
  HRESULT Open(ITfThreadMgr*, TfClientId) noexcept;
  HRESULT ActivateDocument() noexcept;
  DispatchResult Dispatch(const MSG&, const std::shared_ptr<const LayoutTable>&) noexcept;
  DispatchResult DispatchCanonical(const NativeKey&, const std::shared_ptr<const LayoutTable>&) noexcept;
  // pointer 也在同一消息出队点冻结 modifier，不延迟到 WndProc 读取状态。
  bool DispatchPointer(const MSG&) noexcept;
  // own WndProc 在调用 DefWindowProc 前询问；没有第二个 IMM/char 提交出口。
  bool HandleOwnedMessage(UINT, WPARAM, LPARAM) noexcept;
  Fault BeginRevoke() noexcept;
  Fault CoreFenceCompleted(uint64_t fence) noexcept;
  Fault TryConfirmRetired() noexcept;
  Fault BeginContextRetirement(uint64_t end_fence) noexcept;
  FocusState State() const noexcept { return state_; }
  HRESULT RetirementResult() const noexcept { return retirement_result_; }
  void SetGeometry(const SourceGeometry&) noexcept;
  HRESULT Pump() noexcept;
  TextStore* Store() const noexcept { return store_.Get(); }
  ITfContext* Context() const noexcept { return context_.Get(); }
  const ProfileIdentity& Profile() const noexcept { return profile_; }
  const OwnerRef& Owner() const noexcept { return owner_; }
  ULONG References() const noexcept { return references_.load(); }
#ifdef GUOSSH_NATIVE_FIXTURE
  using BoundaryProbe = void (*)(void*, NativeSurface*, unsigned) noexcept;
  void ObserveBoundary(BoundaryProbe probe, void* data) noexcept { probe_ = probe; probe_data_ = data; }
#endif

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID, void**) override;
  ULONG STDMETHODCALLTYPE AddRef() override;
  ULONG STDMETHODCALLTYPE Release() override;
  HRESULT STDMETHODCALLTYPE OnActivated(DWORD, LANGID, REFCLSID, REFGUID, REFGUID,
                                         HKL, DWORD) override;

 private:
  NativeSurface(HWND, OwnerRef, std::shared_ptr<Callbacks>, std::shared_ptr<NativePermit>, NativeOrigin) noexcept;
  ~NativeSurface();
  bool DerivedQueued() noexcept;
  DispatchResult DispatchImpl(const MSG&, const std::shared_ptr<const LayoutTable>&, const NativeKey*) noexcept;
  bool SourceActive(uint64_t event) const noexcept;
  bool DispatchActive(const TextDispatchScope&) const noexcept;
  HRESULT CheckDerived(const TextDispatchScope&, bool*) noexcept;
  DispatchResult AbortDispatch(DispatchResult) noexcept;
  void Boundary(unsigned point) noexcept;
  DispatchResult Translate(const NativeKey&, TextDispatchScope, DispatchResult) noexcept;
  void FailRoute(Fault) noexcept;
  HRESULT DetachNative() noexcept;
  void InvalidateLayout() noexcept;
  static bool IsPointerMessage(UINT) noexcept;

  std::atomic<ULONG> references_{1};
  const DWORD thread_;
  const HWND hwnd_;
  const OwnerRef owner_;
  const NativeOrigin origin_;
  const std::shared_ptr<Callbacks> callbacks_;
  std::shared_ptr<NativePermit> permit_;
  Microsoft::WRL::ComPtr<ITfThreadMgr> manager_;
  Microsoft::WRL::ComPtr<ITfKeystrokeMgr> keys_;
  Microsoft::WRL::ComPtr<ITfDocumentMgr> document_;
  Microsoft::WRL::ComPtr<ITfContext> context_;
  Microsoft::WRL::ComPtr<ITfSource> context_source_, profile_source_;
  Microsoft::WRL::ComPtr<ITfInputProcessorProfileMgr> profiles_;
  Microsoft::WRL::ComPtr<TextStore> store_;
  DWORD edit_cookie_{TF_INVALID_COOKIE}, profile_cookie_{TF_INVALID_COOKIE};
  TfEditCookie owner_cookie_{};
  ProfileIdentity profile_{};
  std::shared_ptr<const LayoutTable> layout_;
  SourceGeometry geometry_{};
  FocusState state_{FocusState::NewEditable};
  uint64_t event_{}, fence_{};
  bool opened_{}, associated_{}, pushed_{}, dispatching_{}, route_disabled_{}, detached_{};
  bool confirming_{}, detaching_{}, revoking_{}, opening_{};
  bool context_only_{};
  HRESULT retirement_result_{S_OK};
#ifdef GUOSSH_NATIVE_FIXTURE
  BoundaryProbe probe_{};
  void* probe_data_{};
#endif
};

// bounded tombstone。slot 持有 source/context 原对象，只有真实 retirement 后可归还。
class ContextPool {
 public:
  HRESULT Create(HWND, OwnerRef, std::shared_ptr<Callbacks>, std::shared_ptr<NativePermit>,
                  NativeSurface**, NativeOrigin = NativeOrigin::VerifiedNative) noexcept;
  size_t Collect() noexcept;
  size_t Occupied() const noexcept;
 private:
  std::array<Microsoft::WRL::ComPtr<NativeSurface>, kContextSlots> slots_{};
  uint64_t greatest_context_{};
};

}  // namespace guossh::native_input
