#pragma once

#include "contracts.h"
#include <textstor.h>
#include <wrl/client.h>
#include <atomic>

namespace guossh::native_input {

// 一个 store 只接受一个原 ticket。封口后必须退役 context，不能重贴新事务。
class TextStore final : public ITextStoreACP,
                        public ITfContextOwnerCompositionSink,
                        public ITfTextEditSink {
 public:
  static TextStore* Create(HWND, OwnerRef, std::shared_ptr<Callbacks>, std::shared_ptr<NativePermit>) noexcept;
  HRESULT Bind(const TicketRef&) noexcept;
  HRESULT BeginDispatch(const TextDispatchScope&) noexcept;
  HRESULT SelectTranslateRoute(const TextDispatchScope&) noexcept;
  bool DispatchActive(const TextDispatchScope&) const noexcept;
  bool Retired() const noexcept { return retired_; }
  HRESULT EndDispatch() noexcept;
  HRESULT AppendTranslated(const WCHAR*, ULONG) noexcept;
  void SetDeadPending(bool value) noexcept { dead_pending_ = value; }
  void Cancel(Fault) noexcept;
  void Retire() noexcept;
  bool CanDetach() const noexcept;
  ULONG References() const noexcept { return references_.load(); }
  bool Bound() const noexcept { return bound_; }
  bool Sealed() const noexcept { return sealed_; }
  bool Composing() const noexcept { return composition_ != nullptr; }
  bool DeadPending() const noexcept { return dead_pending_; }
  uint64_t Revision() const noexcept { return revision_; }
  uint64_t SideEffects() const noexcept { return side_effects_; }
  Fault Failure() const noexcept { return failure_; }
  const TicketRef* OriginalTicket() const noexcept { return bound_ ? &ticket_ : nullptr; }
  void SetGeometry(const SourceGeometry&) noexcept;
  HRESULT SetTextLayout(const RECT* acp_rects, ULONG count, uint64_t edit_revision) noexcept;
  // 消息循环在 RequestLock 调用栈之外执行；不在锁内通知应用改动。
  HRESULT PumpNotifications() noexcept;
#ifdef GUOSSH_NATIVE_FIXTURE
  void ObserveUtf16Reads(ULONG* count) noexcept { examined_ = count; }
#endif

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID, void**) override;
  ULONG STDMETHODCALLTYPE AddRef() override;
  ULONG STDMETHODCALLTYPE Release() override;
  HRESULT STDMETHODCALLTYPE AdviseSink(REFIID, IUnknown*, DWORD) override;
  HRESULT STDMETHODCALLTYPE UnadviseSink(IUnknown*) override;
  HRESULT STDMETHODCALLTYPE RequestLock(DWORD, HRESULT*) override;
  HRESULT STDMETHODCALLTYPE GetStatus(TS_STATUS*) override;
  HRESULT STDMETHODCALLTYPE QueryInsert(LONG, LONG, ULONG, LONG*, LONG*) override;
  HRESULT STDMETHODCALLTYPE GetSelection(ULONG, ULONG, TS_SELECTION_ACP*, ULONG*) override;
  HRESULT STDMETHODCALLTYPE SetSelection(ULONG, const TS_SELECTION_ACP*) override;
  HRESULT STDMETHODCALLTYPE GetText(LONG, LONG, WCHAR*, ULONG, ULONG*, TS_RUNINFO*, ULONG,
                                    ULONG*, LONG*) override;
  HRESULT STDMETHODCALLTYPE SetText(DWORD, LONG, LONG, const WCHAR*, ULONG, TS_TEXTCHANGE*) override;
  HRESULT STDMETHODCALLTYPE GetFormattedText(LONG, LONG, IDataObject**) override;
  HRESULT STDMETHODCALLTYPE GetEmbedded(LONG, REFGUID, REFIID, IUnknown**) override;
  HRESULT STDMETHODCALLTYPE QueryInsertEmbedded(const GUID*, const FORMATETC*, BOOL*) override;
  HRESULT STDMETHODCALLTYPE InsertEmbedded(DWORD, LONG, LONG, IDataObject*, TS_TEXTCHANGE*) override;
  HRESULT STDMETHODCALLTYPE InsertTextAtSelection(DWORD, const WCHAR*, ULONG, LONG*, LONG*,
                                                  TS_TEXTCHANGE*) override;
  HRESULT STDMETHODCALLTYPE InsertEmbeddedAtSelection(DWORD, IDataObject*, LONG*, LONG*,
                                                       TS_TEXTCHANGE*) override;
  HRESULT STDMETHODCALLTYPE RequestSupportedAttrs(DWORD, ULONG, const TS_ATTRID*) override;
  HRESULT STDMETHODCALLTYPE RequestAttrsAtPosition(LONG, ULONG, const TS_ATTRID*, DWORD) override;
  HRESULT STDMETHODCALLTYPE RequestAttrsTransitioningAtPosition(LONG, ULONG, const TS_ATTRID*, DWORD) override;
  HRESULT STDMETHODCALLTYPE FindNextAttrTransition(LONG, LONG, ULONG, const TS_ATTRID*, DWORD,
                                                   LONG*, BOOL*, LONG*) override;
  HRESULT STDMETHODCALLTYPE RetrieveRequestedAttrs(ULONG, TS_ATTRVAL*, ULONG*) override;
  HRESULT STDMETHODCALLTYPE GetEndACP(LONG*) override;
  HRESULT STDMETHODCALLTYPE GetActiveView(TsViewCookie*) override;
  HRESULT STDMETHODCALLTYPE GetACPFromPoint(TsViewCookie, const POINT*, DWORD, LONG*) override;
  HRESULT STDMETHODCALLTYPE GetTextExt(TsViewCookie, LONG, LONG, RECT*, BOOL*) override;
  HRESULT STDMETHODCALLTYPE GetScreenExt(TsViewCookie, RECT*) override;
  HRESULT STDMETHODCALLTYPE GetWnd(TsViewCookie, HWND*) override;
  HRESULT STDMETHODCALLTYPE OnStartComposition(ITfCompositionView*, BOOL*) override;
  HRESULT STDMETHODCALLTYPE OnUpdateComposition(ITfCompositionView*, ITfRange*) override;
  HRESULT STDMETHODCALLTYPE OnEndComposition(ITfCompositionView*) override;
  HRESULT STDMETHODCALLTYPE OnEndEdit(ITfContext*, TfEditCookie, ITfEditRecord*) override;

 private:
  TextStore(HWND, OwnerRef, std::shared_ptr<Callbacks>, std::shared_ptr<NativePermit>) noexcept;
  ~TextStore() = default;
  bool OnThread() const noexcept;
  bool Boundary(LONG) const noexcept;
  bool Range(LONG, LONG) const noexcept;
  bool ReadLocked() const noexcept;
  bool WriteLocked() const noexcept;
  bool Editable() const noexcept;
  HRESULT Replace(LONG, LONG, const WCHAR*, ULONG, TS_TEXTCHANGE*) noexcept;
  HRESULT PublishPreedit() noexcept;
  HRESULT Finalize() noexcept;
  HRESULT ReadCompositionRange(ITfRange*) noexcept;
  HRESULT GrantLock(DWORD) noexcept;

  std::atomic<ULONG> references_{1};
  const DWORD thread_;
  const HWND hwnd_;
  const OwnerRef owner_;
  const std::shared_ptr<Callbacks> callbacks_;
  std::shared_ptr<NativePermit> permit_;
  Microsoft::WRL::ComPtr<ITextStoreACPSink> sink_;
  Microsoft::WRL::ComPtr<IUnknown> sink_identity_;
  Microsoft::WRL::ComPtr<ITfCompositionView> composition_;
  DWORD sink_mask_{}, lock_{}, pending_lock_{};
  bool requesting_{}, notifying_{}, bound_{}, sealed_{}, retired_{}, in_dispatch_{};
  bool dead_pending_{}, end_composition_pending_{}, pending_text_{}, pending_selection_{}, pending_layout_{};
  TicketRef ticket_{};
  TextDispatchScope scope_{};
  PayloadGrant grant_{};
  Fault failure_{Fault::None};
  uint64_t revision_{}, side_effects_{}, published_revision_{};
  std::array<WCHAR, kTextUnits> text_{};
  // UTF-8 转换的同时存活峰值包含在固定 native permit 中。
  std::array<char, kTextBytes> utf8_{};
  LONG length_{}, composing_start_{}, composing_end_{}, frozen_prefix_{};
  TS_SELECTION_ACP selection_{0, 0, {TS_AE_END, FALSE}};
  TS_TEXTCHANGE app_change_{};
  SourceGeometry geometry_{};
  std::array<RECT, kTextUnits + 1> acp_rects_{};
  ULONG layout_count_{};
  uint64_t layout_revision_{};
#ifdef GUOSSH_NATIVE_FIXTURE
  ULONG* examined_{};
#endif
};

}  // namespace guossh::native_input
