#include "window_source.h"

#include <cwchar>
#include <limits>
#include <new>

#include "tsf_probe.h"

namespace guosshell::native_input_spike {
namespace {
struct CreationScope {
  bool& creating;
  ~CreationScope() { creating = false; }
};

bool IsKey(UINT message) noexcept {
  return message == WM_KEYDOWN || message == WM_KEYUP ||
         message == WM_SYSKEYDOWN || message == WM_SYSKEYUP;
}

bool IsCharacterOrIme(UINT message) noexcept {
  return message == WM_CHAR || message == WM_SYSCHAR ||
         message == WM_DEADCHAR || message == WM_SYSDEADCHAR ||
         message == WM_UNICHAR || message == WM_IME_CHAR ||
         message == WM_IME_STARTCOMPOSITION ||
         message == WM_IME_ENDCOMPOSITION || message == WM_IME_COMPOSITION ||
         message == WM_IME_SETCONTEXT || message == WM_IME_NOTIFY ||
         message == WM_IME_REQUEST || message == WM_IME_CONTROL ||
         message == WM_IME_KEYDOWN || message == WM_IME_KEYUP;
}

bool IsPointer(UINT message) noexcept {
  return (message >= WM_MOUSEFIRST && message <= WM_MOUSELAST) ||
         message == WM_POINTERDOWN || message == WM_POINTERUP ||
         message == WM_POINTERUPDATE || message == WM_POINTERENTER ||
         message == WM_POINTERLEAVE || message == WM_POINTERWHEEL ||
         message == WM_POINTERHWHEEL || message == WM_TOUCH;
}
}  // namespace

WindowSource::~WindowSource() {
  const CallScope entry(*this);
  // 调用者必须先完成 Shutdown；兜底仅关闭本窗口，不能当作 Rust 撤权证据。
  if (bound_ && !core_revoked_ && OnThread()) BeginRevoke();
  if (tsf_) tsf_->StopAdmissions();
  DestroyLocalResources();
}

bool WindowSource::OnThread() const noexcept {
  return thread_ == GetCurrentThreadId();
}

HRESULT WindowSource::Create(HWND parent, HINSTANCE instance,
                              const SourceOptions& options) noexcept {
  const CallScope entry(*this);
  if (attempted_) return E_UNEXPECTED;
  if (!options.opt_in) {
    last_stop_ = StopReason::Disabled;
    return S_FALSE;
  }
  attempted_ = true;
  const auto& bridge = options.callbacks;
  DWORD process = 0;
  const DWORD parent_thread = GetWindowThreadProcessId(parent, &process);
  if (!options.dedicated_fixture || !parent ||
      process != GetCurrentProcessId() || parent_thread != GetCurrentThreadId() ||
      !bridge.bind || !bridge.key || !bridge.validate_text ||
      !bridge.cancel_text || !bridge.revoke) {
    last_stop_ = StopReason::InvalidFixture;
    return E_INVALIDARG;
  }
  thread_ = GetCurrentThreadId();
  instance_ = instance;
  options_ = options;
  creating_ = true;
  const CreationScope creation{creating_};
  swprintf_s(class_name_, L"GuoSSHell.WaveA.Input.%p", static_cast<void*>(this));
  WNDCLASSEXW window_class{};
  window_class.cbSize = sizeof(window_class);
  window_class.lpfnWndProc = WindowProc;
  window_class.hInstance = instance_;
  window_class.hCursor = LoadCursor(nullptr, IDC_ARROW);
  window_class.lpszClassName = class_name_;
  window_class_ = RegisterClassExW(&window_class);
  if (!window_class_) {
    last_stop_ = StopReason::OsFailure;
    return HRESULT_FROM_WIN32(GetLastError());
  }
  const HWND created = CreateWindowExW(0, class_name_, L"Wave A 输入探针（不提供文本编辑）",
                          WS_CHILD | WS_VISIBLE | WS_TABSTOP, 8, 8, 620, 160,
                          parent, nullptr, instance_, this);
  if (!created || hwnd_ != created || !IsWindow(created)) {
    const DWORD error = GetLastError();
    const HRESULT hr = stop_latched_ ? E_ABORT : created ? CO_E_OBJNOTCONNECTED
        : error ? HRESULT_FROM_WIN32(error) : E_FAIL;
    DestroyLocalResources();
    last_stop_ = stop_latched_ ? StopReason::Revoked : StopReason::OsFailure;
    return hr;
  }
  if (stop_latched_) {
    DestroyLocalResources();
    last_stop_ = StopReason::Revoked;
    return E_ABORT;
  }
  ContextIdentity bound_identity;
  Admission bound;
  {
    const CallScope external(*this, true);
    bound = bridge.bind(bridge.user, created, &bound_identity);
  }
  bound_ = bound == Admission::Accepted;
  identity_ = bound_identity;
  if (!bound_ || !identity_.owner_cookie || !identity_.context_epoch) {
    last_stop_ = StopReason::InvalidFixture;
    if (bound_) BeginRevoke();
    if (!bound_ || core_revoked_) DestroyLocalResources();
    return E_INVALIDARG;
  }
  // bind 返回的身份必须撤权；创建中的停用与销毁不能被 Accepted 复活。
  if (stop_latched_ || hwnd_ != created || !IsWindow(created)) {
    BeginRevoke();
    if (core_revoked_) DestroyLocalResources();
    last_stop_ = core_revoked_ ? StopReason::Revoked : StopReason::RevokePending;
    return core_revoked_ ? E_ABORT : E_PENDING;
  }
  tsf_ = new (std::nothrow) TsfProbe();
  HRESULT hr;
  {
    const CallScope external(*this, true);
    hr = tsf_ ? tsf_->Initialize(created, identity_) : E_OUTOFMEMORY;
  }
  if (FAILED(hr)) {
    last_stop_ = StopReason::TsfUnavailable;
    BeginRevoke();
    // 不把失败的初始化当作可交接的 editable owner。
    if (core_revoked_ && SUCCEEDED(RetireTsf()) &&
        (!tsf_ || tsf_->CanRelease())) DestroyLocalResources();
    last_stop_ = StopReason::TsfUnavailable;
    return hr;
  }
  if (stop_latched_ || hwnd_ != created || !IsWindow(created)) {
    BeginRevoke();
    if (core_revoked_ && SUCCEEDED(RetireTsf()) && tsf_->CanRelease()) {
      DestroyLocalResources();
    }
    last_stop_ = core_revoked_ ? StopReason::Revoked : StopReason::RevokePending;
    return core_revoked_ ? E_ABORT : E_PENDING;
  }
  accepting_ = true;
  last_stop_ = StopReason::None;
  return S_OK;
}

bool WindowSource::Record(TraceStage stage, uint64_t event, HRESULT hr) noexcept {
  if (trace_size_ == trace_.size()) {
    accepting_ = false;
    if (tsf_) tsf_->StopAdmissions();
    last_stop_ = StopReason::TraceFull;
    return false;
  }
  LARGE_INTEGER now{};
  QueryPerformanceCounter(&now);
  trace_[trace_size_++] = {stage, event, now.QuadPart, hr};
  return true;
}

LoopDecision WindowSource::Block(StopReason reason, HRESULT hr) noexcept {
  last_stop_ = reason;
  return {LoopRoute::Handled, reason, hr};
}

LoopDecision WindowSource::PreTranslate(const MSG& message,
                                        uint32_t candidate) noexcept {
  const CallScope entry(*this);
  // 未创建／未选中的窗口完全走原路线，不观测其 key、text 或 pointer。
  if (!hwnd_ || message.hwnd != hwnd_) return {};
  if (!OnThread()) return Block(StopReason::WrongThread, RPC_E_WRONG_THREAD);
  if (IsCharacterOrIme(message.message)) {
    ++counters_.character_messages_blocked;
    return Block(StopReason::CharacterAssociationUnsupported, E_NOTIMPL);
  }
  if (IsPointer(message.message)) {
    ++counters_.pointer_messages_blocked;
    return Block(StopReason::PointerUnsupported, E_NOTIMPL);
  }
  if (!IsKey(message.message)) return {LoopRoute::DispatchOnly};
  if (!accepting_) return Block(StopReason::Revoked, CO_E_OBJNOTCONNECTED);
  if (GetFocus() != hwnd_) {
    BeginRevoke();
    return Block(StopReason::NoFocus, CO_E_OBJNOTCONNECTED);
  }
  if (next_event_ == (std::numeric_limits<uint64_t>::max)()) {
    BeginRevoke();
    return Block(StopReason::EventIdExhausted, E_UNEXPECTED);
  }
  NativeKeyEvent event{};
  event.context = identity_;
  event.native_event_id = ++next_event_;
  event.message = message.message;
  event.virtual_key = message.wParam;
  event.key_bits = message.lParam;
  event.message_time = message.time;
  event.layout = GetKeyboardLayout(thread_);
  event.keyboard_state_available =
      GetKeyboardState(event.queue_keyboard_state.data()) != FALSE;
  const auto bits = static_cast<uint64_t>(message.lParam);
  event.scan_code = static_cast<uint16_t>((bits >> 16) & 0xff);
  event.extended = (bits & (uint64_t{1} << 24)) != 0;
  const bool up = message.message == WM_KEYUP || message.message == WM_SYSKEYUP;
  event.phase = up ? KeyPhase::Up
                   : (bits & (uint64_t{1} << 30)) ? KeyPhase::Repeat
                                                : KeyPhase::Down;
  event.default_candidate_intent = candidate;

  RejectedKey* latched = nullptr;
  RejectedKey* empty = nullptr;
  for (auto& key : rejected_) {
    if (key.occupied && key.scan == event.scan_code &&
        key.extended == event.extended) latched = &key;
    if (!key.occupied && !empty) empty = &key;
  }
  if (latched && !up) return Block(StopReason::None);
  event.local_rejected_lifecycle = latched != nullptr;
  if (!Record(TraceStage::GateEnter, event.native_event_id)) {
    BeginRevoke();
    return Block(StopReason::TraceFull, E_BOUNDS);
  }
  ++counters_.gate;
  GateReply reply;
  {
    const CallScope external(*this, true);
    reply = options_.callbacks.key(options_.callbacks.user, event);
  }
  if (!Record(TraceStage::GateReturn, event.native_event_id)) {
    BeginRevoke();
    return Block(StopReason::TraceFull, E_BOUNDS);
  }
  if (!accepting_) return Block(StopReason::Revoked, CO_E_OBJNOTCONNECTED);
  if (latched) {
    // Up 仍通知 bridge 释放／取消；其结果不得复活本地 Busy Down。
    latched->occupied = false;
    if (reply.admission == Admission::Accepted &&
        reply.disposition == Disposition::AllowNativeText) {
      BeginRevoke();
      return Block(StopReason::InvalidTicket, E_INVALIDARG);
    }
    return Block(StopReason::None);
  }
  if (reply.admission != Admission::Accepted) {
    if (!up) {
      if (!event.scan_code || !empty) {
        BeginRevoke();
        return Block(StopReason::RejectionLedgerFull,
                     HRESULT_FROM_WIN32(ERROR_NOT_ENOUGH_QUOTA));
      }
      *empty = {true, event.scan_code, event.extended};
    }
    return Block(StopReason::None);
  }
  if (reply.disposition != Disposition::AllowNativeText) {
    if (!Record(TraceStage::Blocked, event.native_event_id)) {
      BeginRevoke();
      return Block(StopReason::TraceFull, E_BOUNDS);
    }
    return Block(StopReason::None);
  }

  // VK_PROCESSKEY／VK_PACKET 或缺 scan 的事件不能补回 pre-IME provenance。
  // 不调用 ToUnicodeEx／ImmGetVirtualKey，也不在本机生成 raw key 映射。
  const bool trustworthy = reply.raw_proof == RawProof::Trusted &&
                           event.scan_code && event.keyboard_state_available &&
                           event.virtual_key != VK_PROCESSKEY &&
                           event.virtual_key != VK_PACKET;
  const auto& permit = reply.text;
  bool valid = trustworthy && SameContext(permit.context, identity_) &&
               permit.native_event_id == event.native_event_id && permit.ticket;
  if (valid) {
    if (!Record(TraceStage::TicketValidate, event.native_event_id)) {
      BeginRevoke();
      return Block(StopReason::TraceFull, E_BOUNDS);
    }
    const CallScope external(*this, true);
    valid = options_.callbacks.validate_text(options_.callbacks.user, event,
                                             permit);
  }
  if (!valid) {
    BeginRevoke();
    return Block(trustworthy ? StopReason::InvalidTicket : StopReason::UnknownRaw,
                 E_INVALIDARG);
  }
  // validate callback 允许重入撤权；不得拿返回的旧 ticket 继续 TSF。
  if (!accepting_) return Block(StopReason::Revoked, CO_E_OBJNOTCONNECTED);
  BOOL tested = FALSE;
  if (!Record(up ? TraceStage::TestKeyUp : TraceStage::TestKeyDown,
              event.native_event_id)) {
    BeginRevoke();
    return Block(StopReason::TraceFull, E_BOUNDS);
  }
  if (up) ++counters_.tsf_test_up;
  else ++counters_.tsf_test_down;
  HRESULT hr;
  {
    const CallScope external(*this, true);
    hr = tsf_->TestKey(message, up, &tested);
  }
  if (!accepting_) {
    if (core_revoked_) RetireTsf();
    return Block(StopReason::Revoked, CO_E_OBJNOTCONNECTED);
  }
  if (SUCCEEDED(hr) && tested) {
    if (Record(up ? TraceStage::KeyUp : TraceStage::KeyDown,
               event.native_event_id)) {
      if (up) ++counters_.tsf_up;
      else ++counters_.tsf_down;
      BOOL eaten = FALSE;
      const CallScope external(*this, true);
      hr = tsf_->Key(message, up, &eaten);
    } else {
      BeginRevoke();
      return Block(StopReason::TraceFull, E_BOUNDS);
    }
  }
  if (!accepting_) {
    if (core_revoked_) RetireTsf();
    return Block(StopReason::Revoked, CO_E_OBJNOTCONNECTED);
  }
  // Wave A 无可用 ACP commit，原预约必须取消，绝不 Translate 造第二条 char。
  if (!Record(TraceStage::TicketCancel, event.native_event_id, hr)) {
    BeginRevoke();
    return Block(StopReason::TraceFull, E_BOUNDS);
  }
  bool cancelled;
  {
    const CallScope external(*this, true);
    cancelled = options_.callbacks.cancel_text(options_.callbacks.user, permit);
  }
  if (!cancelled) {
    BeginRevoke();
    return Block(StopReason::TicketCancelPending, E_PENDING);
  }
  if (!accepting_) return Block(StopReason::Revoked, CO_E_OBJNOTCONNECTED);
  return Block(FAILED(hr) ? StopReason::TsfUnavailable
                          : StopReason::TextStoreUnsupported,
               FAILED(hr) ? hr : E_NOTIMPL);
}

RevokeAck WindowSource::BeginRevoke() noexcept {
  const CallScope entry(*this);
  if (thread_ && !OnThread()) {
    last_stop_ = StopReason::WrongThread;
    return RevokeAck::Failed;
  }
  stop_latched_ = true;
  accepting_ = false;
  if (tsf_) tsf_->StopAdmissions();
  // 身份尚未返回时只能锁存停用，不能提前确认不存在的 owner 已撤权。
  if (!bound_) return creating_ ? RevokeAck::Pending : RevokeAck::Confirmed;
  if (revoke_started_) return revoke_ack_;
  revoke_started_ = true;
  Record(TraceStage::RevokeEnter, 0);
  RevokeAck ack;
  {
    const CallScope external(*this, true);
    ack = options_.callbacks.revoke(options_.callbacks.user, identity_);
  }
  if (!core_revoked_) revoke_ack_ = ack;
  Record(TraceStage::RevokeReturn, 0);
  if (revoke_ack_ == RevokeAck::Confirmed) {
    core_revoked_ = true;
    RetireTsf();
  } else {
    last_stop_ = StopReason::RevokePending;
  }
  return revoke_ack_;
}

HRESULT WindowSource::RetireTsf() noexcept {
  if (external_depth_) return E_PENDING;
  const CallScope external(*this, true);
  const HRESULT hr = tsf_ ? tsf_->Retire() : S_OK;
  if (FAILED(hr)) last_stop_ = StopReason::TsfUnavailable;
  return hr;
}

HRESULT WindowSource::ConfirmRevoke() noexcept {
  const CallScope entry(*this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (!revoke_started_) return E_UNEXPECTED;
  core_revoked_ = true;
  revoke_ack_ = RevokeAck::Confirmed;
  return RetireTsf();
}

HRESULT WindowSource::NotifyHostDeactivated() noexcept {
  const CallScope entry(*this);
  return BeginRevoke() == RevokeAck::Confirmed ? S_OK : E_PENDING;
}

HRESULT WindowSource::Shutdown() noexcept {
  const CallScope entry(*this);
  if (thread_ && !OnThread()) return RPC_E_WRONG_THREAD;
  if (entry_depth_ > 1 || external_depth_ || creating_) {
    BeginRevoke();
    last_stop_ = StopReason::InFlightCallPending;
    return E_PENDING;
  }
  if (!attempted_ || (!hwnd_ && !bound_ && !tsf_ && !window_class_)) return S_OK;
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (BeginRevoke() != RevokeAck::Confirmed) return E_PENDING;
  HRESULT hr = RetireTsf();
  if (FAILED(hr)) return hr;
  if (tsf_ && !tsf_->CanRelease()) {
    last_stop_ = StopReason::RetiredReferencesPending;
    return E_PENDING;
  }
  if (hwnd_) {
    // 保留旧 HWND 直到其有限队列被清空，避免已排队 MSG 命中回收后的句柄。
    // 超额交给下一轮正常 loop 处理，不同步无限 drain／等待。
    MSG stale{};
    for (size_t i = 0; i < kRetirementDrainSlots; ++i) {
      if (!PeekMessageW(&stale, hwnd_, 0, 0, PM_REMOVE)) break;
    }
    if (PeekMessageW(&stale, hwnd_, 0, 0, PM_NOREMOVE)) {
      last_stop_ = StopReason::RetiredMessagesPending;
      return E_PENDING;
    }
  }
  return DestroyLocalResources();
}

HRESULT WindowSource::AcquireTextStoreForProbe(IUnknown** store) noexcept {
  const CallScope entry(*this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  const CallScope external(*this, true);
  return tsf_ ? tsf_->AcquireStore(store) : CO_E_OBJNOTCONNECTED;
}

LONG WindowSource::LiveTextStores() noexcept { return TsfProbe::LiveStores(); }

HRESULT WindowSource::DestroyLocalResources() noexcept {
  accepting_ = false;
  TsfProbe* retiring = tsf_;
  tsf_ = nullptr;
  {
    const CallScope external(*this, true);
    delete retiring;
  }
  if (hwnd_) {
    if (!DestroyWindow(hwnd_)) {
      last_stop_ = StopReason::OsFailure;
      return HRESULT_FROM_WIN32(GetLastError());
    }
    hwnd_ = nullptr;
  }
  if (window_class_) {
    if (!UnregisterClassW(class_name_, instance_)) {
      last_stop_ = StopReason::OsFailure;
      return HRESULT_FROM_WIN32(GetLastError());
    }
    window_class_ = 0;
  }
  return S_OK;
}

LRESULT CALLBACK WindowSource::WindowProc(HWND hwnd, UINT message,
                                         WPARAM wparam, LPARAM lparam) noexcept {
  WindowSource* source = reinterpret_cast<WindowSource*>(
      GetWindowLongPtrW(hwnd, GWLP_USERDATA));
  if (message == WM_NCCREATE) {
    const auto* create = reinterpret_cast<const CREATESTRUCTW*>(lparam);
    source = static_cast<WindowSource*>(create->lpCreateParams);
  }
  if (!source) return DefWindowProcW(hwnd, message, wparam, lparam);
  const CallScope entry(*source);
  if (message == WM_NCCREATE) {
    source->hwnd_ = hwnd;
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(source));
  }
  return source->OnMessage(message, wparam, lparam);
}

LRESULT WindowSource::OnMessage(UINT message, WPARAM wparam,
                                LPARAM lparam) noexcept {
  if (message == WM_SETFOCUS && accepting_ && tsf_) {
    HRESULT hr;
    {
      const CallScope external(*this, true);
      hr = tsf_->Focus();
    }
    if (FAILED(hr)) {
      BeginRevoke();
      last_stop_ = StopReason::TsfUnavailable;
    }
    return 0;
  }
  if (message == WM_KILLFOCUS) {
    BeginRevoke();
    return 0;
  }
  if (message == WM_DESTROY) {
    BeginRevoke();
    return 0;
  }
  if (message == WM_NCDESTROY) {
    const HWND destroyed = hwnd_;
    SetWindowLongPtrW(destroyed, GWLP_USERDATA, 0);
    hwnd_ = nullptr;
    return DefWindowProcW(destroyed, message, wparam, lparam);
  }
  if (IsKey(message)) {
    ++counters_.missing_pretranslate;
    BeginRevoke();
    last_stop_ = StopReason::MissingPreTranslate;
    return 0;
  }
  if (IsCharacterOrIme(message)) {
    ++counters_.character_messages_blocked;
    last_stop_ = StopReason::CharacterAssociationUnsupported;
    return 0;
  }
  if (IsPointer(message)) {
    ++counters_.pointer_messages_blocked;
    last_stop_ = StopReason::PointerUnsupported;
    return 0;
  }
  if (message == WM_PAINT) {
    PAINTSTRUCT paint{};
    HDC dc = BeginPaint(hwnd_, &paint);
    FillRect(dc, &paint.rcPaint, GetSysColorBrush(COLOR_WINDOW));
    EndPaint(hwnd_, &paint);
    return 0;
  }
  return DefWindowProcW(hwnd_, message, wparam, lparam);
}

}  // namespace guosshell::native_input_spike
