#include "host_internal.h"
#include <windowsx.h>

namespace guosshell::desktop_native_input {
PaneRoot::PaneRoot(NativeHostState& host, uint64_t id, std::string token, uint32_t session,
    uint64_t generation, std::string identity, uint64_t attempt, uint64_t lifecycle)
    : token_(std::move(token)), id_(id), attempt_(attempt), lifecycle_(lifecycle), session_(session),
      generation_(generation), identity_(std::move(identity)), host_(host) {}
PaneRoot::~PaneRoot() {
  if (hwnd_) { SetWindowLongPtrW(hwnd_, GWLP_USERDATA, 0); RemovePropW(hwnd_, GUOSSH_NORMAL_WINDOWS_INPUT_PROPERTY); DestroyWindow(hwnd_); }
}
GuosshNormalContext PaneRoot::Context() const noexcept {
  auto context = Abi<GuosshNormalContext>();
  context.handle = bound_.handle; context.session_id = session_; context.generation = generation_;
  context.identity = reinterpret_cast<const uint8_t*>(identity_.data()); context.identity_len = identity_.size();
  context.attempt = attempt_; context.focus_epoch = bound_.focus_epoch;
  return context;
}
Map PaneRoot::Reply(const char* state) const {
  auto result = State(state);
  result[Value("paneToken")] = Value(token_);
  Put(result, "attempt", attempt_); Put(result, "focusEpoch", bound_.focus_epoch);
  if (!failure_.empty()) result[Value("reason")] = Value(failure_);
  return result;
}
void PaneRoot::NotifyState(const char* state) {
  const bool quiet_before_release = std::string_view(state) == "nativequiet" && want_release_;
  auto result = Reply(state); Put(result, "requestId", quiet_before_release ? 0 : request_);
  if (!quiet_before_release && std::string_view(state) != "pending" && std::string_view(state) != "inactive") request_ = 0;
  host_.Emit("paneState", std::move(result));
}
void PaneRoot::Fail(const char* reason) {
  if (!failed_) {
    failed_ = true; failure_ = reason; want_focus_ = false;
    Seal(); NotifyState("failed");
  }
}
bool PaneRoot::Create(const Geometry& geometry) {
  WNDCLASSW wc{}; wc.hInstance = GetModuleHandleW(nullptr); wc.lpfnWndProc = WindowProc;
  wc.lpszClassName = L"GuoSSHell.NormalInput.Pane.V1"; wc.hCursor = LoadCursorW(nullptr, IDC_IBEAM);
  WNDCLASSW existing{};
  if (!RegisterClassW(&wc) && (!GetClassInfoW(wc.hInstance, wc.lpszClassName, &existing) || existing.lpfnWndProc != WindowProc)) return false;
  opening_ = true;
  hwnd_ = CreateWindowExW(WS_EX_TRANSPARENT, wc.lpszClassName, L"", WS_CHILD,
      0, 0, 1, 1, host_.view, nullptr, wc.hInstance, this);
  opening_ = false;
  if (!hwnd_ || !SetPropW(hwnd_, GUOSSH_NORMAL_WINDOWS_INPUT_PROPERTY, this)) return false;
  geometry_ = geometry;
  opening_ = true;
  const bool positioned = geometry_.Position(hwnd_, host_.view);
  opening_ = false;
  return positioned && OpenContext();
}
bool PaneRoot::OpenContext() {
  if (surface_ || next_context_ == UINT64_MAX || revoking_ || released_) return false;
  auto permit = Permit::Acquire(host_.budget);
  if (!permit) return false;
  ni::OwnerRef owner{id_, id_, session_, generation_, id_, ++next_context_};
  if (FAILED(pool_.Create(hwnd_, owner, shared_from_this(), std::move(permit), &surface_, ni::NativeOrigin::VerifiedNative))) return false;
  opening_ = true;
  const HRESULT opened = surface_->Open(host_.manager.Get(), host_.client);
  opening_ = false;
  if (FAILED(opened)) return false;
  layout_ = host_.worker.Result(surface_->Profile());
  layout_requested_ = false;
  if (!layout_) {
    auto layout_permit = Permit::Acquire(host_.budget);
    if (layout_permit) layout_requested_ = host_.worker.Request(surface_->Profile(), ni::LayoutTable::Create(std::move(layout_permit)));
  }
  ApplyGeometry();
  return true;
}
bool PaneRoot::Focus() {
  if (Active()) { want_focus_ = false; NotifyState("active"); return true; }
  if (!want_focus_ || revoking_ || failed_ || !surface_ || !host_.NativeQuiet(this)) return false;
  if (GetTickCount64() >= deadline_ || GetForegroundWindow() != GetAncestor(host_.view, GA_ROOT)) {
    Fail("FOCUS_INTENT_EXPIRED"); return false;
  }
  ShowWindow(hwnd_, SW_SHOWNOACTIVATE);
  if (revoking_ || failed_ || !want_focus_) return false;
  SetFocus(hwnd_);
  if (GetFocus() != hwnd_ || FAILED(surface_->ActivateDocument())) { Fail("FOCUS_UNAVAILABLE"); return false; }
  if (revoking_ || failed_ || !want_focus_ || GetFocus() != hwnd_) return false;
  if (!bound_.handle) {
    auto registration = Abi<GuosshNormalRegistration>();
    registration.session_id = session_; registration.generation = generation_;
    registration.identity = reinterpret_cast<const uint8_t*>(identity_.data()); registration.identity_len = identity_.size();
    registration.attempt = attempt_; registration.window = reinterpret_cast<uintptr_t>(hwnd_);
    registration.input_object = reinterpret_cast<uintptr_t>(this);
    const uint32_t status = host_.api.register_trusted_host(&registration, &bound_);
    if (status == GUOSSH_NORMAL_BUSY) return false;
    if (status != GUOSSH_NORMAL_OK || !bound_.handle || bound_.attempt != attempt_ || !bound_.focus_epoch) {
      Fail("REGISTRATION_REJECTED"); return false;
    }
    registration_handle_ = bound_.handle;
    core_revoked_ = false;
  }
  if (!bound_confirmed_) {
    auto context = Context(); auto callback = Abi<GuosshNormalBound>();
    const auto status = host_.api.bind(&context, &callback);
    if (status == GUOSSH_NORMAL_BUSY) return false;
    if (status != GUOSSH_NORMAL_OK || !callback.handle || callback.attempt != attempt_ || callback.focus_epoch != bound_.focus_epoch) {
      Fail("BINDING_REJECTED"); return false;
    }
    bound_ = callback;
    bound_confirmed_ = true;
  }
  if (revoking_ || failed_ || !want_focus_) return false;
  if (host_.raw_counter == UINT64_MAX) { Fail("SOURCE_EPOCH_EXHAUSTED"); return false; }
  raw_epoch_ = ++host_.raw_counter;
  const auto raw_status = host_.raw.Enter(hwnd_, raw_epoch_);
  raw_restore_required_ = host_.raw.LastRegisteredEpoch() == raw_epoch_;
  if (raw_status != RawStatus::Ready) { Fail("RAW_SOURCE_UNAVAILABLE"); return false; }
  if (revoking_ || failed_ || !want_focus_) { Seal(); return false; }
  active_ = true; want_focus_ = false;
  NotifyState("active");
  return true;
}
void PaneRoot::Seal(const char* reason) {
  if (released_ || quiet_) return;
  if (reason && failure_.empty()) failure_ = reason;
  const bool was_active = active_;
  active_ = false; want_focus_ = false;
  if (!revoking_) { revoking_ = true; deadline_ = GetTickCount64() + kTransitionMilliseconds; }
  geometry_.valid = false;
  if (surface_) surface_->SetGeometry({});
  if (host_.raw.Owns(hwnd_)) {
    const auto status = host_.raw.Leave();
    if (status != RawStatus::Ready) failure_ = "RAW_RESTORATION_UNCONFIRMED";
  }
  ReleasePointerCapture();
  ShowWindow(hwnd_, SW_HIDE);
  if (was_active) NotifyState("inactive");
}
bool PaneRoot::QueryEnd() {
  if (!bound_.handle) return true;
  auto context = Context(); auto end = Abi<GuosshNormalTextEnd>();
  const auto status = host_.api.text_end_query(&context, &end);
  if (status != GUOSSH_NORMAL_OK) return false;
  if (ending_.end_fence && end.end_fence &&
      (end.end_fence != ending_.end_fence || end.context_id != ending_.context_id || end.transaction_id != ending_.transaction_id)) return false;
  ending_ = end;
  return true;
}
bool PaneRoot::AckEnd(uint64_t context_id, uint64_t fence) {
  if (!fence) return ending_.end_fence == 0;
  if (ending_.context_id != context_id || ending_.end_fence != fence) return false;
  auto context = Context();
  if (host_.api.text_end_ack(&context, &ending_) != GUOSSH_NORMAL_OK) return false;
  ended_[ended_index_] = ending_;
  ended_index_ = (ended_index_ + 1) % ended_.size();
  ending_ = Abi<GuosshNormalTextEnd>();
  return true;
}
void PaneRoot::Poll() {
  if (polling_ || released_ || opening_) return;
  polling_ = true;
  struct Exit { bool& flag; ~Exit() { flag = false; } } exit{polling_};
  if (((revoking_ && !quiet_) || (surface_ && surface_->State() == ni::FocusState::NativeRetiring)) && deadline_ && GetTickCount64() >= deadline_) {
    if (!failed_) { failed_ = true; failure_ = "NATIVE_RETIREMENT_TIMEOUT"; NotifyState("failed"); }
    return;
  }
  if (revoking_ && !core_revoked_) {
    if (bound_.handle) {
      auto context = Context();
      const auto status = host_.api.revoke(&context);
      if (status == GUOSSH_NORMAL_BUSY) return;
      if (status != GUOSSH_NORMAL_OK) { Fail("REVOKE_REJECTED"); return; }
    }
    core_revoked_ = true;
  }
  if (revoking_ && raw_restore_required_ && host_.raw.RetirementReady(hwnd_)) raw_restore_required_ = false;
  if (revoking_ && claim_ && !claim_delivered_) {
    auto context = Context(); auto ack = Abi<GuosshNormalAck>(); ack.claim = claim_; ack.kind = 2;
    if (host_.api.ack(&context, &ack) == GUOSSH_NORMAL_OK) { claim_ = claim_effect_ = 0; claim_token_.clear(); }
  }
  if (!QueryEnd()) return;
  if (surface_) {
    if (ending_.end_fence) {
      if (surface_->Owner().context != ending_.context_id) { Fail("TEXT_CONTEXT_MISMATCH"); return; }
      if (surface_->State() == ni::FocusState::Active) {
        deadline_ = GetTickCount64() + kTransitionMilliseconds;
        surface_->BeginContextRetirement(ending_.end_fence);
      }
    } else if (revoking_) {
      if (surface_->State() == ni::FocusState::Active) surface_->BeginContextRetirement(0);
      else if (surface_->State() == ni::FocusState::NewEditable || surface_->State() == ni::FocusState::CoreRevoking) surface_->BeginRevoke();
    } else if (surface_->Store() && surface_->Store()->Sealed() && surface_->State() == ni::FocusState::Active) {
      deadline_ = GetTickCount64() + kTransitionMilliseconds;
      surface_->BeginContextRetirement(0);
    }
    surface_->Pump();
    if (surface_->State() == ni::FocusState::Retired) {
      if (composition_ticket_) EmitComposition(true);
      surface_.Reset(); pool_.Collect(); composition_.clear(); composition_ticket_ = composition_revision_ = 0;
      if (!revoking_ && !failed_) {
        if (!OpenContext() || FAILED(surface_->ActivateDocument())) { Fail("CONTEXT_ROTATION_FAILED"); return; }
        deadline_ = 0;
      }
    } else if (surface_->State() == ni::FocusState::Active && !layout_) {
      layout_ = host_.worker.Result(surface_->Profile());
      if (!layout_ && !layout_requested_) {
        auto permit = Permit::Acquire(host_.budget);
        if (permit) layout_requested_ = host_.worker.Request(surface_->Profile(), ni::LayoutTable::Create(std::move(permit)));
      }
    }
  }
  pool_.Collect();
  if (revoking_ && core_revoked_ && !surface_ && !pool_.Occupied() && !ending_.end_fence && !raw_restore_required_) {
    if (!quiet_) { quiet_ = true; NotifyState("nativequiet"); }
    if (want_release_) Release();
  }
}
bool PaneRoot::Release() {
  if (released_) return true;
  if (!quiet_ || claim_ || surface_ || pool_.Occupied()) return false;
  if (!release_deadline_) release_deadline_ = GetTickCount64() + kTransitionMilliseconds;
  if (GetTickCount64() >= release_deadline_) { Fail("ROOT_RELEASE_TIMEOUT"); return false; }
  if (bound_.handle) {
    auto context = Context();
    auto status = host_.api.release_retired(&context);
    if (status == GUOSSH_NORMAL_BUSY) return false;
    if (status != GUOSSH_NORMAL_OK) { Fail("ROOT_RELEASE_REJECTED"); return false; }
    if (registration_handle_ && registration_handle_ != bound_.handle) {
      bound_.handle = registration_handle_;
      bound_confirmed_ = false;
      context = Context();
      status = host_.api.release_retired(&context);
      if (status == GUOSSH_NORMAL_BUSY) return false;
      if (status != GUOSSH_NORMAL_OK) { Fail("REGISTRATION_RELEASE_REJECTED"); return false; }
    }
    bound_.handle = 0;
    registration_handle_ = 0;
  }
  released_ = true;
  RemovePropW(hwnd_, GUOSSH_NORMAL_WINDOWS_INPUT_PROPERTY);
  DestroyWindow(hwnd_); hwnd_ = nullptr;
  NotifyState("released");
  return true;
}
bool PaneRoot::CoreRevoke(const ni::OwnerRef&, uint64_t* fence) noexcept {
  try {
    Seal();
    if (!core_revoked_) {
      auto context = Context();
      if (bound_.handle && host_.api.revoke(&context) != GUOSSH_NORMAL_OK) return false;
      core_revoked_ = true;
    }
    if (!QueryEnd()) return false;
    *fence = id_;
    return true;
  } catch (...) { return false; }
}
bool PaneRoot::ConfirmRetired(const ni::OwnerRef& owner, uint64_t fence) noexcept {
  if (!core_revoked_ || fence != id_) return false;
  return !ending_.end_fence || AckEnd(owner.context, ending_.end_fence);
}
bool PaneRoot::ContextQuiet(const ni::OwnerRef& owner, const ni::TicketRef*, uint64_t fence) noexcept {
  return AckEnd(owner.context, fence);
}
LRESULT CALLBACK PaneRoot::WindowProc(HWND window, UINT message, WPARAM wp, LPARAM lp) {
  auto* self = reinterpret_cast<PaneRoot*>(GetWindowLongPtrW(window, GWLP_USERDATA));
  if (message == WM_NCCREATE) {
    self = static_cast<PaneRoot*>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams);
    SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
  }
  if (!self) return DefWindowProcW(window, message, wp, lp);
  auto keep = self->shared_from_this();
  try {
    if (message == WM_PAINT) { PAINTSTRUCT paint{}; BeginPaint(window, &paint); EndPaint(window, &paint); return 0; }
    if (message == WM_ERASEBKGND) return 1;
    if (message == WM_NCHITTEST) return HTCLIENT;
    if (message == WM_GETDLGCODE) return DLGC_WANTALLKEYS | DLGC_WANTCHARS;
    if (message == WM_MOUSEACTIVATE) return MA_NOACTIVATE;
    if (message == WM_CAPTURECHANGED && self->pointer_capture_ && reinterpret_cast<HWND>(lp) != window) {
      self->pointer_capture_ = false; self->Seal("POINTER_CAPTURE_LOST");
    }
    if (message == WM_KILLFOCUS || (message == WM_ACTIVATEAPP && !wp)) self->Seal();
    if (!self->opening_ && (message == WM_MOVE || message == WM_SIZE || message == WM_DPICHANGED)) self->Invalidate();
    if (self->surface_ && self->surface_->HandleOwnedMessage(message, wp, lp)) return 0;
    if (TextMessage(message)) return 0;
  } catch (...) { self->active_ = false; self->revoking_ = true; self->failed_ = true; }
  // WM_INPUT 未经主循环消费时仍交系统清理，不存在第二 payload 路径。
  return DefWindowProcW(window, message, wp, lp);
}
void PaneRoot::LegacyMessage(const MSG& message) {
  if (surface_) surface_->HandleOwnedMessage(message.message, message.wParam, message.lParam);
}
void PaneRoot::ReleasePointerCapture() noexcept {
  pointer_capture_ = false;
  if (GetCapture() == hwnd_) ReleaseCapture();
}
}  // namespace guosshell::desktop_native_input
