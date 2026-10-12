#include "native_surface.h"

#include <windowsx.h>
#include <new>

namespace guossh::native_input {
using Microsoft::WRL::ComPtr;
namespace {
struct FlagScope {
  explicit FlagScope(bool& value) noexcept : value(value) { value = true; }
  ~FlagScope() { value = false; }
  bool& value;
};
}

NativeSurface::NativeSurface(HWND hwnd, OwnerRef owner, std::shared_ptr<Callbacks> callbacks,
                             std::shared_ptr<NativePermit> permit, NativeOrigin origin) noexcept
    : thread_(GetCurrentThreadId()), hwnd_(hwnd), owner_(owner), origin_(origin),
      callbacks_(std::move(callbacks)), permit_(std::move(permit)) {}
NativeSurface* NativeSurface::Create(HWND hwnd, OwnerRef owner, std::shared_ptr<Callbacks> callbacks,
                                     std::shared_ptr<NativePermit> permit, NativeOrigin origin) noexcept {
  if (!permit) return nullptr;
  void* memory = permit->Memory().Allocate(sizeof(NativeSurface));
  if (!memory) return nullptr;
  return ::new (memory) NativeSurface(hwnd, owner, std::move(callbacks), std::move(permit), origin);
}
NativeSurface::~NativeSurface() {
  // 正常调用方在退休后释放；异常初始化也只解除本对象注册。
  if (store_) store_->Retire();
  DetachNative();
}
HRESULT NativeSurface::QueryInterface(REFIID iid, void** out) {
  if (!out) return E_POINTER;
  *out = nullptr;
  if (iid != IID_IUnknown && iid != IID_ITfInputProcessorProfileActivationSink) return E_NOINTERFACE;
  *out = static_cast<ITfInputProcessorProfileActivationSink*>(this);
  AddRef();
  return S_OK;
}
ULONG NativeSurface::AddRef() { return ++references_; }
ULONG NativeSurface::Release() {
  ULONG count = --references_;
  if (!count) {
    auto permit = std::move(permit_);
    this->~NativeSurface();
    permit->Memory().Deallocate(this, sizeof(NativeSurface));
  }
  return count;
}
HRESULT NativeSurface::Open(ITfThreadMgr* manager, TfClientId client) noexcept {
  ComPtr<NativeSurface> self(this);
  if (thread_ != GetCurrentThreadId()) return RPC_E_WRONG_THREAD;
  if (opened_ || opening_ || state_ != FocusState::NewEditable || manager_ || !manager || !callbacks_ || !permit_ || !owner_.context ||
      !owner_.source || !IsWindow(hwnd_) ||
      GetWindowThreadProcessId(hwnd_, nullptr) != thread_) return E_INVALIDARG;
  FlagScope opening(opening_);
  manager_ = manager;
  HRESULT hr = manager_->QueryInterface(IID_PPV_ARGS(&keys_));
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  store_.Attach(TextStore::Create(hwnd_, owner_, callbacks_, permit_));
  if (!store_) return E_OUTOFMEMORY;
  if (state_ != FocusState::NewEditable) { store_->Retire(); return E_ABORT; }
  hr = manager_->CreateDocumentMgr(&document_);
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  hr = document_->CreateContext(client, 0, static_cast<ITextStoreACP*>(store_.Get()), &context_, &owner_cookie_);
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  hr = context_->QueryInterface(IID_PPV_ARGS(&context_source_));
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  hr = context_source_->AdviseSink(IID_ITfTextEditSink, static_cast<ITfTextEditSink*>(store_.Get()), &edit_cookie_);
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  hr = document_->Push(context_.Get());
  if (FAILED(hr)) return hr;
  pushed_ = true;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  ComPtr<ITfDocumentMgr> previous;
  hr = manager_->AssociateFocus(hwnd_, document_.Get(), &previous);
  if (FAILED(hr)) return hr;
  associated_ = true;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  if (previous) {
    ComPtr<ITfDocumentMgr> ignored;
    manager_->AssociateFocus(hwnd_, previous.Get(), &ignored);
    associated_ = false;
    return E_ACCESSDENIED;
  }
  hr = CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr, CLSCTX_INPROC_SERVER,
                         IID_PPV_ARGS(&profiles_));
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  hr = manager_->QueryInterface(IID_PPV_ARGS(&profile_source_));
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  hr = ReadActiveProfile(profiles_.Get(), 1, &profile_);
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  hr = profile_source_->AdviseSink(IID_ITfInputProcessorProfileActivationSink,
      static_cast<ITfInputProcessorProfileActivationSink*>(this), &profile_cookie_);
  if (FAILED(hr)) return hr;
  if (state_ != FocusState::NewEditable) return E_ABORT;
  opened_ = true;
  state_ = FocusState::Active;
  return S_OK;
}
HRESULT NativeSurface::ActivateDocument() noexcept {
  ComPtr<NativeSurface> self(this);
  if (GetCurrentThreadId() != thread_) return RPC_E_WRONG_THREAD;
  if (!opened_ || state_ != FocusState::Active ||
      (origin_ == NativeOrigin::VerifiedNative && GetFocus() != hwnd_)) return E_ACCESSDENIED;
  // 不调用 SetForegroundWindow/SetFocus；宿主只在自己的终端已获焦时调用。
  auto manager = manager_;
  auto document = document_;
  HRESULT hr = manager->SetFocus(document.Get());
  return state_ == FocusState::Active && !detached_ ? hr : E_ABORT;
}
bool NativeSurface::DerivedQueued() noexcept {
  MSG msg{};
  const BOOL chars = PeekMessageW(&msg, hwnd_, WM_CHAR, WM_DEADCHAR, PM_NOREMOVE);
  Boundary(5);
  if (chars) return true;
  const BOOL system_chars = PeekMessageW(&msg, hwnd_, WM_SYSCHAR, WM_SYSDEADCHAR, PM_NOREMOVE);
  Boundary(5);
  return system_chars != FALSE;
}
void NativeSurface::Boundary(unsigned point) noexcept {
#ifdef GUOSSH_NATIVE_FIXTURE
  if (probe_) probe_(probe_data_, this, point);
#else
  (void)point;
#endif
}
bool NativeSurface::SourceActive(uint64_t event) const noexcept {
  return GetCurrentThreadId() == thread_ && opened_ && state_ == FocusState::Active &&
         !route_disabled_ && !detached_ && dispatching_ && event_ == event &&
         store_ && !store_->Retired() && IsWindow(hwnd_) &&
         (origin_ == NativeOrigin::DeclaredSynthetic || GetFocus() == hwnd_);
}
bool NativeSurface::DispatchActive(const TextDispatchScope& scope) const noexcept {
  return SourceActive(scope.event_id) && scope.context == owner_.context && store_->DispatchActive(scope);
}
HRESULT NativeSurface::CheckDerived(const TextDispatchScope& scope, bool* queued) noexcept {
  *queued = false;
  for (const UINT first : {WM_CHAR, WM_SYSCHAR}) {
    if (!DispatchActive(scope)) return E_ABORT;
    MSG message{};
    const BOOL found = PeekMessageW(&message, hwnd_, first, first + 1, PM_NOREMOVE);
    Boundary(1);
    if (!DispatchActive(scope)) return E_ABORT;
    if (found) { *queued = true; break; }
  }
  return S_OK;
}
DispatchResult NativeSurface::AbortDispatch(DispatchResult result) noexcept {
  store_->Cancel(Fault::TextContextUnverified);
  store_->EndDispatch();
  result.fault = Fault::TextContextUnverified;
  result.native_result = E_ABORT;
  return result;
}
void NativeSurface::FailRoute(Fault fault) noexcept {
  route_disabled_ = true;
  if (store_) store_->Cancel(fault);
  callbacks_->Faulted(owner_, fault);
}
DispatchResult NativeSurface::Dispatch(const MSG& msg, const std::shared_ptr<const LayoutTable>& layout) noexcept {
  ComPtr<NativeSurface> self(this);
  DispatchResult result{};
  if (msg.hwnd != hwnd_ || (msg.message != WM_KEYDOWN && msg.message != WM_KEYUP &&
      msg.message != WM_SYSKEYDOWN && msg.message != WM_SYSKEYUP)) return result;
  result.owned = true;
  if (GetCurrentThreadId() != thread_ || !opened_ || state_ != FocusState::Active || dispatching_) {
    result.fault = Fault::TextContextUnverified;
    return result;
  }
  if (origin_ == NativeOrigin::VerifiedNative && GetFocus() != hwnd_) {
    BeginRevoke();
    result.fault = Fault::TextContextUnverified;
    return result;
  }
  if (event_ == UINT64_MAX) { result.fault = Fault::ContextBudget; return result; }
  FlagScope dispatch(dispatching_);
  const uint64_t event = ++event_;
  if (layout) {
    if ((layout_ && layout_.get() != layout.get()) || !SameProfile(layout->Profile(), profile_)) {
      InvalidateLayout();
      result.fault = Fault::LayoutStale;
      return result;
    }
    layout_ = layout;
  }
  NativeKey key{};
  ProfileIdentity now{};
  auto profiles = profiles_;
  auto keys = keys_;
  HRESULT hr = ReadActiveProfile(profiles.Get(), profile_.epoch, &now);
  if (!SourceActive(event)) { result.fault = Fault::TextContextUnverified; return result; }
  if (FAILED(hr) || !SameProfile(now, profile_)) {
    InvalidateLayout();
    result.fault = Fault::LayoutStale;
    return result;
  }
  hr = FreezeKey(hwnd_, msg, event, profile_, &key);
  if (!SourceActive(event)) { result.fault = Fault::TextContextUnverified; return result; }
  if (FAILED(hr)) { result.native_result = hr; result.fault = Fault::LayoutUnverified; return result; }
  if (layout_ && !key.trusted_raw) key.metadata_fault = layout_->Lookup(&key);
  const GateReceipt receipt = callbacks_->Gate(owner_, key, store_->OriginalTicket());
  result.disposition = receipt.disposition;
  if (receipt.disposition != Disposition::AllowNativeText) {
    return result;
  }
  // 注入 gate 也不能将 unknown raw 升格；真实父 gate 仍为唯一绑定 authority。
  if (!key.trusted_raw || !receipt.trusted_raw || !SourceActive(event)) {
    result.fault = key.metadata_fault == Fault::None ? Fault::TextContextUnverified : key.metadata_fault;
    if (receipt.ticket.context == owner_.context && receipt.ticket.owner_generation == owner_.generation)
      callbacks_->Cancel(receipt.ticket, result.fault);
    return result;
  }
  if (receipt.lifecycle_only) {
    // 只有原已闭合 ticket 的 Up 可平衡 TSF 状态；store 已只读，不能开新事务。
    if (!key.up || !store_->Sealed() || !store_->OriginalTicket() ||
        store_->Failure() != Fault::None || !(receipt.ticket == *store_->OriginalTicket())) {
      result.fault = Fault::InvalidIdentity;
      return result;
    }
    BOOL eaten{};
    result.native_result = keys->TestKeyUp(msg.wParam, msg.lParam, &eaten);
    ++result.tsf_calls;
    if (!SourceActive(event) || store_->Failure() != Fault::None) { result.fault = Fault::TextContextUnverified; return result; }
    if (SUCCEEDED(result.native_result) && eaten) {
      result.native_result = keys->KeyUp(msg.wParam, msg.lParam, &eaten);
      ++result.tsf_calls;
      if (!SourceActive(event) || store_->Failure() != Fault::None) { result.fault = Fault::TextContextUnverified; return result; }
    }
    return result;
  }
  hr = store_->Bind(receipt.ticket);
  TextDispatchScope scope{receipt.ticket, owner_.context, key.event_id, TextRoute::TsfStore};
  if (!SourceActive(event)) return AbortDispatch(result);
  if (SUCCEEDED(hr)) hr = store_->BeginDispatch(scope);
  if (FAILED(hr)) {
    result.native_result = hr;
    result.fault = Fault::TextContextUnverified;
    return result;
  }
  bool queued_before{};
  if (FAILED(CheckDerived(scope, &queued_before))) return AbortDispatch(result);
  if (queued_before) {
    FailRoute(Fault::AmbiguousTextRoute);
    store_->EndDispatch();
    result.fault = Fault::AmbiguousTextRoute;
    return result;
  }
  const uint64_t before = store_->SideEffects();
  BOOL tested{}, eaten{};
  if (!DispatchActive(scope)) return AbortDispatch(result);
  hr = key.up ? keys->TestKeyUp(msg.wParam, msg.lParam, &tested)
              : keys->TestKeyDown(msg.wParam, msg.lParam, &tested);
  ++result.tsf_calls;
  Boundary(2);
  if (!DispatchActive(scope)) return AbortDispatch(result);
  if (SUCCEEDED(hr) && tested) {
    hr = key.up ? keys->KeyUp(msg.wParam, msg.lParam, &eaten)
                : keys->KeyDown(msg.wParam, msg.lParam, &eaten);
    ++result.tsf_calls;
    Boundary(3);
    if (!DispatchActive(scope)) return AbortDispatch(result);
  }
  bool queued_after{};
  if (FAILED(CheckDerived(scope, &queued_after))) return AbortDispatch(result);
  if (FAILED(hr) || (!eaten && store_->SideEffects() != before) || queued_after) {
    FailRoute(Fault::AmbiguousTextRoute);
    result.fault = Fault::AmbiguousTextRoute;
    result.native_result = FAILED(hr) ? hr : E_UNEXPECTED;
    store_->EndDispatch();
  } else if (eaten || store_->Composing()) {
    result.route = TextRoute::TsfStore;
    result.native_result = store_->EndDispatch();
  } else {
    // 保持同一 scope，不能先 EndDispatch 再把已封口 ticket 用于 Translate。
    scope.route = TextRoute::TranslateStore;
    result = Translate(key, scope, result);
  }
  if (store_->Failure() != Fault::None && result.fault == Fault::None) result.fault = store_->Failure();
  return result;
}
DispatchResult NativeSurface::Translate(const NativeKey& key, TextDispatchScope scope,
                                         DispatchResult result) noexcept {
  result.route = TextRoute::TranslateStore;
  if (!DispatchActive(scope)) return AbortDispatch(result);
  if (key.legacy.kind == MetadataKind::Dead || key.raw.kind == MetadataKind::Dead) {
    FailRoute(Fault::DeadStateRetirementUnproven);
    result.fault = Fault::DeadStateRetirementUnproven;
    store_->EndDispatch();
    return result;
  }
  HRESULT hr = store_->SelectTranslateRoute(scope);
  if (FAILED(hr)) {
    FailRoute(Fault::AmbiguousTextRoute);
    result.fault = Fault::AmbiguousTextRoute;
    store_->EndDispatch();
    return result;
  }
  if (!DispatchActive(scope)) return AbortDispatch(result);
  if (key.up) { result.native_result = store_->EndDispatch(); return result; }
  const bool system = key.message.message == WM_SYSKEYDOWN;
  TranslateMessage(&key.message);
  ++result.translates;
  if (!DispatchActive(scope)) return AbortDispatch(result);
  const UINT first = system ? WM_SYSCHAR : WM_CHAR;
  const UINT last = system ? WM_SYSDEADCHAR : WM_DEADCHAR;
  std::array<WCHAR, kTextUnits> chars{};
  ULONG count{};
  WCHAR pending_high{};
  ULONG pending_repeats{};
  MSG derived{};
  for (UINT i = 0; i < kDrainLimit; ++i) {
    const BOOL found = PeekMessageW(&derived, hwnd_, first, last, PM_REMOVE);
    Boundary(4);
    if (found && (derived.message == WM_DEADCHAR || derived.message == WM_SYSDEADCHAR))
      store_->SetDeadPending(true);
    if (!DispatchActive(scope)) return AbortDispatch(result);
    if (!found) break;
    ++result.derived;
    const LPARAM identity_mask = (0xffLL << 16) | (1LL << 24) | (1LL << 29);
    if ((derived.lParam & identity_mask) != (key.message.lParam & identity_mask) ||
        derived.wParam > 0xffff || LOWORD(derived.lParam) != LOWORD(key.message.lParam)) {
      result.fault = Fault::AmbiguousTextRoute;
      break;
    }
    if (derived.message == WM_DEADCHAR || derived.message == WM_SYSDEADCHAR) {
      // 未证的 kernel dead state 不尝试清除，阻止 retirement/new editable。
      store_->SetDeadPending(true);
      result.fault = Fault::DeadStateRetirementUnproven;
      break;
    }
    const ULONG repeats = LOWORD(derived.lParam);
    const WCHAR unit = static_cast<WCHAR>(derived.wParam);
    if (!repeats) { result.fault = Fault::AmbiguousTextRoute; break; }
    if (LowSurrogate(unit)) {
      if (!pending_high || repeats != pending_repeats) { result.fault = Fault::InvalidUtf16; break; }
      if (repeats > (kTextUnits - count) / 2) { result.fault = Fault::TextCapacity; break; }
      for (ULONG n = 0; n < repeats; ++n) { chars[count++] = pending_high; chars[count++] = unit; }
      pending_high = 0;
    } else {
      if (pending_high) { result.fault = Fault::InvalidUtf16; break; }
      if (HighSurrogate(unit)) { pending_high = unit; pending_repeats = repeats; }
      else {
        if (repeats > kTextUnits - count) { result.fault = Fault::TextCapacity; break; }
        for (ULONG n = 0; n < repeats; ++n) chars[count++] = unit;
      }
    }
  }
  if (pending_high) result.fault = Fault::InvalidUtf16;
  bool queued{};
  if (FAILED(CheckDerived(scope, &queued))) return AbortDispatch(result);
  if (queued) result.fault = Fault::AmbiguousTextRoute;
  if (result.fault != Fault::None) FailRoute(result.fault);
  else {
    result.native_result = store_->AppendTranslated(chars.data(), count);
    if (FAILED(result.native_result)) {
      result.fault = Fault::AmbiguousTextRoute;
      FailRoute(result.fault);
    }
  }
  hr = store_->EndDispatch();
  if (SUCCEEDED(result.native_result)) result.native_result = hr;
  return result;
}
void NativeSurface::SetGeometry(const SourceGeometry& geometry) noexcept {
  if (GetCurrentThreadId() != thread_ || !store_) return;
  geometry_ = geometry;
  if (geometry.revision.source != owner_.source) geometry_.acknowledged = false;
  store_->SetGeometry(geometry_);
}
bool NativeSurface::IsPointerMessage(UINT message) noexcept {
  switch (message) {
    case WM_MOUSEMOVE: case WM_LBUTTONDOWN: case WM_LBUTTONUP: case WM_LBUTTONDBLCLK:
    case WM_RBUTTONDOWN: case WM_RBUTTONUP: case WM_RBUTTONDBLCLK:
    case WM_MBUTTONDOWN: case WM_MBUTTONUP: case WM_MOUSEWHEEL: case WM_MOUSEHWHEEL: return true;
    default: return false;
  }
}
bool NativeSurface::DispatchPointer(const MSG& msg) noexcept {
  ComPtr<NativeSurface> self(this);
  if (msg.hwnd != hwnd_ || GetCurrentThreadId() != thread_ || !IsPointerMessage(msg.message)) return false;
  const UINT message = msg.message;
  const WPARAM wparam = msg.wParam;
  const LPARAM lparam = msg.lParam;
  if (origin_ == NativeOrigin::VerifiedNative && GetFocus() != hwnd_) return true;
  if (state_ != FocusState::Active || !geometry_.acknowledged ||
      geometry_.revision.source != owner_.source || geometry_.cell_width <= 0 || geometry_.cell_height <= 0)
    return true;
  POINT point{GET_X_LPARAM(lparam), GET_Y_LPARAM(lparam)};
  if (message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL) ScreenToClient(hwnd_, &point);
  POINT screen{point.x + geometry_.client_origin_screen.x, point.y + geometry_.client_origin_screen.y};
  if (!PtInRect(&geometry_.screen_clip, screen)) return true;
  PointerEvent event{};
  event.message = message;
  event.client = point;
  event.buttons = GET_KEYSTATE_WPARAM(wparam) & (MK_LBUTTON | MK_RBUTTON | MK_MBUTTON | MK_XBUTTON1 | MK_XBUTTON2);
  // Shift/Ctrl 来自同消息，Alt/Win 来自当前出队的线程态；不读 Flutter latest state。
  event.modifiers = (wparam & MK_SHIFT ? 1u : 0u) | (wparam & MK_CONTROL ? 2u : 0u);
  std::array<BYTE, 256> state{};
  if (!GetKeyboardState(state.data())) return true;
  if (state[VK_MENU] & 0x80) event.modifiers |= 4;
  if ((state[VK_LWIN] | state[VK_RWIN]) & 0x80) event.modifiers |= 8;
  event.wheel_delta = message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL ? GET_WHEEL_DELTA_WPARAM(wparam) : 0;
  event.geometry = geometry_.revision;
  event.column = (screen.x - geometry_.screen_clip.left) / geometry_.cell_width;
  event.row = (screen.y - geometry_.screen_clip.top) / geometry_.cell_height;
  if (event.column < 0 || event.row < 0 || event.column >= geometry_.columns || event.row >= geometry_.rows) return true;
  callbacks_->Pointer(owner_, event);
  return true;
}
bool NativeSurface::HandleOwnedMessage(UINT message, WPARAM wparam, LPARAM lparam) noexcept {
  ComPtr<NativeSurface> self(this);
  if (GetCurrentThreadId() != thread_) return false;
  if (message == WM_CHAR || message == WM_SYSCHAR || message == WM_DEADCHAR ||
      message == WM_SYSDEADCHAR || message == WM_IME_CHAR || message == WM_IME_COMPOSITION ||
      message == WM_IME_STARTCOMPOSITION || message == WM_IME_ENDCOMPOSITION) {
    if (store_ && (message == WM_DEADCHAR || message == WM_SYSDEADCHAR)) store_->SetDeadPending(true);
    if (message == WM_CHAR || message == WM_SYSCHAR || message == WM_DEADCHAR || message == WM_SYSDEADCHAR)
      FailRoute(Fault::AmbiguousTextRoute);
    return true;
  }
  if (message == WM_INPUTLANGCHANGE) { InvalidateLayout(); return true; }
  if (message == WM_KILLFOCUS || (message == WM_ACTIVATEAPP && !wparam)) BeginRevoke();
  if (message == WM_SIZE || message == WM_MOVE || message == WM_DPICHANGED) {
    geometry_.acknowledged = false;
    if (store_) store_->SetGeometry(geometry_);
  }
  (void)lparam;
  // 主循环已经处理 pointer，WndProc 只阻止第二消费者。
  return IsPointerMessage(message);
}
void NativeSurface::InvalidateLayout() noexcept {
  if (profile_.epoch != UINT64_MAX) ++profile_.epoch;
  route_disabled_ = true;
  callbacks_->Faulted(owner_, Fault::LayoutStale);
  BeginRevoke();
}
HRESULT NativeSurface::OnActivated(DWORD, LANGID, REFCLSID, REFGUID category, REFGUID, HKL, DWORD) {
  ComPtr<NativeSurface> self(this);
  if (GetCurrentThreadId() != thread_) return RPC_E_WRONG_THREAD;
  if (IsEqualGUID(category, GUID_TFCAT_TIP_KEYBOARD) && opened_) InvalidateLayout();
  return S_OK;
}
Fault NativeSurface::BeginRevoke() noexcept {
  ComPtr<NativeSurface> self(this);
  if (GetCurrentThreadId() != thread_) return Fault::TextContextUnverified;
  if (state_ == FocusState::Retired) return Fault::None;
  if (state_ == FocusState::NativeRetiring) return TryConfirmRetired();
  if (revoking_) return Fault::NativeRetirementPending;
  FlagScope revoking(revoking_);
  state_ = FocusState::CoreRevoking;
  // Busy 不等待、不先 retire native；父在 core fence 完成时回送原 fence。
  if (!callbacks_->CoreRevoke(owner_, &fence_)) return Fault::Busy;
  if (state_ == FocusState::Retired) return Fault::None;
  if (state_ == FocusState::NativeRetiring) return TryConfirmRetired();
  return CoreFenceCompleted(fence_);
}
Fault NativeSurface::CoreFenceCompleted(uint64_t fence) noexcept {
  ComPtr<NativeSurface> self(this);
  if (GetCurrentThreadId() != thread_ || state_ != FocusState::CoreRevoking || !fence ||
      (fence_ && fence != fence_)) return Fault::InvalidIdentity;
  fence_ = fence;
  state_ = FocusState::NativeRetiring;
  if (store_) store_->Retire();
  if (dispatching_ || (store_ && !store_->CanDetach() && !store_->Composing()))
    return Fault::NativeRetirementPending;
  return TryConfirmRetired();
}
HRESULT NativeSurface::DetachNative() noexcept {
  if (GetCurrentThreadId() != thread_) return RPC_E_WRONG_THREAD;
  if (detached_) return S_OK;
  if (dispatching_ || detaching_ || opening_) return E_PENDING;
  FlagScope detaching(detaching_);
  if (context_ && store_ && store_->Composing()) {
    ComPtr<ITfContextOwnerCompositionServices> services;
    HRESULT hr = context_->QueryInterface(IID_PPV_ARGS(&services));
    if (FAILED(hr)) return hr;
    hr = services->TerminateComposition(nullptr);
    if (FAILED(hr)) return hr;
  }
  if (store_ && !store_->CanDetach()) return E_PENDING;
  if (manager_ && document_) {
    ComPtr<ITfDocumentMgr> focus;
    HRESULT hr = manager_->GetFocus(&focus);
    if (FAILED(hr)) return hr;
    if (focus.Get() == document_.Get()) {
      hr = manager_->SetFocus(nullptr);
      if (FAILED(hr)) return hr;
    }
  }
  if (associated_) {
    ComPtr<ITfDocumentMgr> previous;
    HRESULT hr = manager_->AssociateFocus(hwnd_, nullptr, &previous);
    if (FAILED(hr)) return hr;
    associated_ = false;
  }
  if (profile_source_ && profile_cookie_ != TF_INVALID_COOKIE) {
    HRESULT hr = profile_source_->UnadviseSink(profile_cookie_);
    if (FAILED(hr)) return hr;
    profile_cookie_ = TF_INVALID_COOKIE;
  }
  if (context_source_ && edit_cookie_ != TF_INVALID_COOKIE) {
    HRESULT hr = context_source_->UnadviseSink(edit_cookie_);
    if (FAILED(hr)) return hr;
    edit_cookie_ = TF_INVALID_COOKIE;
  }
  if (pushed_) {
    HRESULT hr = document_->Pop(TF_POPF_ALL);
    if (FAILED(hr)) return hr;
    pushed_ = false;
  }
  context_source_.Reset();
  context_.Reset();
  document_.Reset();
  profile_source_.Reset();
  profiles_.Reset();
  keys_.Reset();
  manager_.Reset();
  detached_ = true;
  return S_OK;
}
Fault NativeSurface::TryConfirmRetired() noexcept {
  ComPtr<NativeSurface> self(this);
  if (GetCurrentThreadId() != thread_) return Fault::TextContextUnverified;
  if (state_ == FocusState::Retired) return Fault::None;
  if (state_ != FocusState::NativeRetiring || confirming_) return Fault::NativeRetirementPending;
  FlagScope confirming(confirming_);
  if (FAILED(DetachNative()) || (store_ && (!store_->CanDetach() || store_->References() != 1)))
    return Fault::NativeRetirementPending;
  const uint64_t original_source = owner_.source, original_context = owner_.context, original_fence = fence_;
  TextStore* const original_store = store_.Get();
  const bool queued = DerivedQueued();
  // Peek 可同步分发已发送消息；队列为空不代表原 store 仍已静默。
  if (queued || state_ != FocusState::NativeRetiring || owner_.source != original_source ||
      owner_.context != original_context || fence_ != original_fence || store_.Get() != original_store ||
      !detached_ || dispatching_ || detaching_ || opening_ ||
      (store_ && (!store_->Retired() || !store_->CanDetach() || store_->References() != 1)))
    return Fault::NativeRetirementPending;
  if (!callbacks_->ConfirmRetired(owner_, fence_)) return Fault::Busy;
  state_ = FocusState::Retired;
  return Fault::None;
}
HRESULT NativeSurface::Pump() noexcept {
  ComPtr<NativeSurface> self(this);
  if (GetCurrentThreadId() != thread_) return RPC_E_WRONG_THREAD;
  if (state_ == FocusState::NativeRetiring) return TryConfirmRetired() == Fault::None ? S_OK : E_PENDING;
  return store_ ? store_->PumpNotifications() : E_UNEXPECTED;
}
HRESULT ContextPool::Create(HWND hwnd, OwnerRef owner, std::shared_ptr<Callbacks> callbacks,
                             std::shared_ptr<NativePermit> permit, NativeSurface** result,
                             NativeOrigin origin) noexcept {
  if (!result) return E_POINTER;
  *result = nullptr;
  if (!owner.context || owner.context <= greatest_context_) return E_ACCESSDENIED;
  for (auto& slot : slots_) {
    if (!slot) {
      slot.Attach(NativeSurface::Create(hwnd, owner, std::move(callbacks), std::move(permit), origin));
      if (!slot) return E_OUTOFMEMORY;
      greatest_context_ = owner.context;
      *result = slot.Get();
      (*result)->AddRef();
      return S_OK;
    }
  }
  return HRESULT_FROM_WIN32(ERROR_NOT_ENOUGH_QUOTA);
}
size_t ContextPool::Collect() noexcept {
  for (auto& slot : slots_)
    if (slot && slot->State() == FocusState::Retired && slot->References() == 1) slot.Reset();
  return Occupied();
}
size_t ContextPool::Occupied() const noexcept {
  size_t count = 0;
  for (const auto& slot : slots_) count += slot != nullptr;
  return count;
}

}  // namespace guossh::native_input
