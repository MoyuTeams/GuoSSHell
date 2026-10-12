#include "window_source.h"
#include "tsf_probe.h"

#include <textstor.h>

#include <cstdio>
#include <cwchar>
#include <new>

namespace spike = guosshell::native_input_spike;

namespace {
constexpr UINT kBarrier = WM_APP + 41;
constexpr UINT_PTR kTimeout = 42;
int failures = 0;
unsigned checks = 0;

void Check(unsigned test, bool passed) noexcept {
  ++checks;
  std::printf("case=%u result=%s\n", test, passed ? "PASS" : "FAIL");
  if (!passed) ++failures;
}

enum class Mode { Consume, Reject, Busy, Allow, WrongContext, UnknownRaw,
                  CancelFails, ReentrantRevoke };
enum class BindAction { None, Revoke, Deactivate, Destroy, Shutdown, KillFocus };
enum class ShutdownAction { None, Key, Validate, Cancel, Revoke };

struct Fixture {
  spike::ContextIdentity identity{71, 1};
  spike::WindowSource* source = nullptr;
  Mode mode = Mode::Consume;
  spike::RevokeAck ack = spike::RevokeAck::Confirmed;
  unsigned binds = 0;
  unsigned keys = 0;
  unsigned validations = 0;
  unsigned cancels = 0;
  unsigned revokes = 0;
  unsigned late_keys = 0;
  uint32_t candidate = 0;
  bool stopped = false;
  BindAction bind_action = BindAction::None;
  ShutdownAction shutdown_action = ShutdownAction::None;
  HRESULT nested_shutdown = E_UNEXPECTED;
  HRESULT nested_deactivate = E_UNEXPECTED;
  spike::RevokeAck nested_revoke = spike::RevokeAck::Failed;
  bool shutdown_called = false;
};

spike::Admission Bind(void* user, HWND hwnd,
                      spike::ContextIdentity* identity) noexcept {
  auto& fixture = *static_cast<Fixture*>(user);
  ++fixture.binds;
  DWORD process = 0;
  GetWindowThreadProcessId(hwnd, &process);
  if (!IsWindow(hwnd) || process != GetCurrentProcessId()) {
    return spike::Admission::InvalidSource;
  }
  *identity = fixture.identity;
  switch (fixture.bind_action) {
    case BindAction::Revoke:
      fixture.nested_revoke = fixture.source->BeginRevoke();
      break;
    case BindAction::Deactivate:
      fixture.nested_deactivate = fixture.source->NotifyHostDeactivated();
      break;
    case BindAction::Destroy: DestroyWindow(hwnd); break;
    case BindAction::Shutdown:
      fixture.nested_shutdown = fixture.source->Shutdown();
      break;
    case BindAction::KillFocus: SendMessageW(hwnd, WM_KILLFOCUS, 0, 0); break;
    default: break;
  }
  return spike::Admission::Accepted;
}

void NestedShutdown(Fixture& fixture, ShutdownAction action) noexcept {
  if (fixture.shutdown_action != action || fixture.shutdown_called) return;
  fixture.shutdown_called = true;
  if (action == ShutdownAction::Revoke) fixture.source->ConfirmRevoke();
  fixture.nested_shutdown = fixture.source->Shutdown();
}

spike::GateReply Key(void* user, const spike::NativeKeyEvent& event) noexcept {
  auto& fixture = *static_cast<Fixture*>(user);
  ++fixture.keys;
  NestedShutdown(fixture, ShutdownAction::Key);
  if (fixture.stopped) ++fixture.late_keys;
  fixture.candidate = event.default_candidate_intent;
  spike::GateReply reply;
  if (!spike::SameContext(event.context, fixture.identity)) return reply;
  reply.admission = spike::Admission::Accepted;
  if (fixture.mode == Mode::Consume) {
    reply.disposition = spike::Disposition::Consumed;
  } else if (fixture.mode == Mode::Reject || event.local_rejected_lifecycle) {
    reply.disposition = spike::Disposition::Rejected;
  } else if (fixture.mode == Mode::Busy) {
    reply.admission = spike::Admission::Busy;
  } else {
    reply.disposition = spike::Disposition::AllowNativeText;
    // 只为合成 case 注入证明；不声称已有真实 raw provenance／Rust ticket。
    reply.raw_proof = fixture.mode == Mode::UnknownRaw
                          ? spike::RawProof::Unknown : spike::RawProof::Trusted;
    reply.text = {event.context, event.native_event_id,
                  9000 + event.native_event_id};
    if (fixture.mode == Mode::WrongContext) ++reply.text.context.context_epoch;
  }
  return reply;
}

bool Validate(void* user, const spike::NativeKeyEvent& event,
              const spike::TextPermit& permit) noexcept {
  auto& fixture = *static_cast<Fixture*>(user);
  ++fixture.validations;
  NestedShutdown(fixture, ShutdownAction::Validate);
  if (fixture.mode == Mode::ReentrantRevoke) fixture.source->BeginRevoke();
  return spike::SameContext(permit.context, fixture.identity) &&
         permit.native_event_id == event.native_event_id &&
         permit.ticket == 9000 + event.native_event_id;
}

bool Cancel(void* user, const spike::TextPermit& permit) noexcept {
  auto& fixture = *static_cast<Fixture*>(user);
  ++fixture.cancels;
  NestedShutdown(fixture, ShutdownAction::Cancel);
  return fixture.mode != Mode::CancelFails &&
         spike::SameContext(permit.context, fixture.identity);
}

spike::RevokeAck Revoke(void* user, spike::ContextIdentity identity) noexcept {
  auto& fixture = *static_cast<Fixture*>(user);
  ++fixture.revokes;
  fixture.stopped = true;
  NestedShutdown(fixture, ShutdownAction::Revoke);
  return spike::SameContext(identity, fixture.identity)
             ? fixture.ack : spike::RevokeAck::Failed;
}

spike::SourceOptions Options(Fixture& fixture) noexcept {
  spike::SourceOptions options;
  options.opt_in = true;
  options.dedicated_fixture = true;
  options.callbacks = {&fixture, Bind, Key, Validate, Cancel, Revoke};
  return options;
}

struct Host {
  spike::WindowSource* source = nullptr;
  unsigned translated_owned_keys = 0;
  unsigned dispatched_owned_keys = 0;
  unsigned character_arrivals = 0;
  bool timed_out = false;
};

LRESULT CALLBACK HostProc(HWND hwnd, UINT message, WPARAM wparam,
                          LPARAM lparam) noexcept {
  auto* host = reinterpret_cast<Host*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
  if (message == WM_NCCREATE) {
    host = static_cast<Host*>(reinterpret_cast<CREATESTRUCTW*>(lparam)->lpCreateParams);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(host));
  }
  if (host && message == WM_ACTIVATEAPP && !wparam && host->source) {
    host->source->NotifyHostDeactivated();
  }
  if (host && message == WM_TIMER && wparam == kTimeout) {
    host->timed_out = true;
    return 0;
  }
  return DefWindowProcW(hwnd, message, wparam, lparam);
}

bool PumpBarrier(HWND root, Host& host) noexcept {
  if (!PostMessageW(root, kBarrier, 0, 0)) return false;
  if (!SetTimer(root, kTimeout, 5000, nullptr)) return false;
  host.timed_out = false;
  MSG message{};
  while (!host.timed_out) {
    const BOOL status = GetMessageW(&message, nullptr, 0, 0);
    if (status <= 0) {
      KillTimer(root, kTimeout);
      return false;
    }
    if (message.hwnd == root && message.message == kBarrier) {
      KillTimer(root, kTimeout);
      return true;
    }
    const bool owned = host.source && message.hwnd == host.source->hwnd();
    const bool key = message.message == WM_KEYDOWN || message.message == WM_KEYUP ||
                     message.message == WM_SYSKEYDOWN || message.message == WM_SYSKEYUP;
    if (owned && (message.message == WM_CHAR || message.message == WM_SYSCHAR ||
                  message.message == WM_IME_CHAR)) ++host.character_arrivals;
    const spike::LoopDecision decision = host.source
        ? host.source->PreTranslate(message, 9) : spike::LoopDecision{};
    if (decision.route == spike::LoopRoute::Handled) continue;
    if (decision.route == spike::LoopRoute::ExistingRoute) {
      if (owned && key) ++host.translated_owned_keys;
      TranslateMessage(&message);
    }
    if (owned && key) ++host.dispatched_owned_keys;
    DispatchMessageW(&message);
  }
  KillTimer(root, kTimeout);
  return false;
}

bool Drain(HWND root, Host& host) noexcept {
  // 第二个 barrier 观察可能由第一轮 Translate 派生到队尾的 char。
  return PumpBarrier(root, host) && PumpBarrier(root, host);
}

bool QueueKey(HWND hwnd, bool up, bool repeat = false,
              unsigned scan_override = 0) noexcept {
  const UINT scan = scan_override ? scan_override
      : MapVirtualKeyExW('A', MAPVK_VK_TO_VSC, GetKeyboardLayout(0));
  if (!scan) return false;
  uint64_t bits = uint64_t{1} | (static_cast<uint64_t>(scan & 0xff) << 16);
  if (repeat || up) bits |= uint64_t{1} << 30;
  if (up) bits |= uint64_t{1} << 31;
  // 仅 PostMessage 到本次创建的 HWND；不是 SendInput／物理键证据。
  return PostMessageW(hwnd, up ? WM_KEYUP : WM_KEYDOWN, 'A',
                      static_cast<LPARAM>(bits)) != FALSE;
}

bool Start(HWND root, HINSTANCE instance, Host& host,
           spike::WindowSource& source, Fixture& fixture) noexcept {
  fixture.source = &source;
  host.source = &source;
  const HRESULT hr = source.Create(root, instance, Options(fixture));
  std::printf("fixture=%llu create_hr=%08lx\n", fixture.identity.context_epoch,
              static_cast<unsigned long>(hr));
  if (FAILED(hr)) return false;
  SetFocus(source.hwnd());
  return GetFocus() == source.hwnd() && source.accepting();
}

bool Ordered(const spike::WindowSource& source) noexcept {
  uint64_t gate = 0;
  uint64_t returned = 0;
  uint64_t validated = 0;
  uint64_t tested = 0;
  LONGLONG previous_ticks = 0;
  for (size_t i = 0; i < source.trace_size(); ++i) {
    const auto& row = source.trace()[i];
    if (row.ticks < previous_ticks) return false;
    previous_ticks = row.ticks;
    switch (row.stage) {
      case spike::TraceStage::GateEnter: gate = row.event; break;
      case spike::TraceStage::GateReturn:
        if (gate != row.event) return false;
        returned = row.event;
        break;
      case spike::TraceStage::TicketValidate:
        if (returned != row.event) return false;
        validated = row.event;
        break;
      case spike::TraceStage::TestKeyDown:
      case spike::TraceStage::TestKeyUp:
        if (gate != row.event || returned != row.event ||
            validated != row.event) return false;
        tested = row.event;
        break;
      case spike::TraceStage::KeyDown:
      case spike::TraceStage::KeyUp:
        if (tested != row.event) return false;
        break;
      default: break;
    }
  }
  return true;
}

void Finish(Host& host, spike::WindowSource& source, unsigned test) noexcept {
  const HWND old = source.hwnd();
  Check(test, source.Shutdown() == S_OK && !IsWindow(old) &&
              spike::WindowSource::LiveTextStores() == 0);
  host.source = nullptr;
}

void BasicCases(HWND root, HINSTANCE instance, Host& host) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  if (!Start(root, instance, host, source, fixture)) {
    Check(10, false);
    host.source = nullptr;
    return;
  }
  bool queued = QueueKey(source.hwnd(), false) &&
                QueueKey(source.hwnd(), false, true) && QueueKey(source.hwnd(), true);
  Check(11, queued && Drain(root, host) && fixture.keys == 3 &&
              fixture.candidate == 9 && source.counters().tsf_test_down == 0 &&
              source.counters().tsf_test_up == 0 && host.character_arrivals == 0 &&
              host.translated_owned_keys == 0 && host.dispatched_owned_keys == 0);
  fixture.mode = Mode::Reject;
  queued = QueueKey(source.hwnd(), false) && QueueKey(source.hwnd(), true);
  Check(12, queued && Drain(root, host) && fixture.keys == 5 &&
              source.counters().tsf_test_down == 0 && host.character_arrivals == 0);
  fixture.mode = Mode::Busy;
  queued = QueueKey(source.hwnd(), false) &&
           QueueKey(source.hwnd(), false, true) && QueueKey(source.hwnd(), true);
  Check(13, queued && Drain(root, host) && fixture.keys == 7 &&
              source.counters().tsf_test_down == 0 && source.accepting());
  fixture.mode = Mode::Allow;
  queued = QueueKey(source.hwnd(), false) && QueueKey(source.hwnd(), true);
  Check(14, queued && Drain(root, host) && fixture.validations == 2 &&
              fixture.cancels == 2 && source.counters().tsf_test_down == 1 &&
              source.counters().tsf_test_up == 1 && Ordered(source) &&
              source.last_stop() == spike::StopReason::TextStoreUnsupported &&
              host.translated_owned_keys == 0 && host.dispatched_owned_keys == 0 &&
              host.character_arrivals == 0);
  std::printf("tsf_test_down=%u tsf_down=%u tsf_test_up=%u tsf_up=%u trace=%zu\n",
              source.counters().tsf_test_down, source.counters().tsf_down,
              source.counters().tsf_test_up, source.counters().tsf_up,
              source.trace_size());

  IUnknown* unknown = nullptr;
  ITextStoreACP* store = nullptr;
  HRESULT hr = source.AcquireTextStoreForProbe(&unknown);
  if (SUCCEEDED(hr)) hr = unknown->QueryInterface(IID_PPV_ARGS(&store));
  if (unknown) unknown->Release();
  HRESULT session = S_OK;
  Check(15, SUCCEEDED(hr) && store &&
              store->RequestLock(TS_LF_READWRITE | TS_LF_SYNC, &session) == E_NOTIMPL &&
              session == E_NOTIMPL);
  SetFocus(root);
  const unsigned before = fixture.keys;
  queued = QueueKey(source.hwnd(), false);
  Check(16, fixture.revokes == 1 && source.core_revoked() && !source.accepting() &&
              queued && Drain(root, host) && fixture.keys == before && fixture.late_keys == 0);
  TS_TEXTCHANGE change{};
  session = S_OK;
  Check(17, store &&
              store->SetText(0, 0, 0, L"x", 1, &change) == CO_E_OBJNOTCONNECTED &&
              store->RequestLock(TS_LF_READ, &session) == CO_E_OBJNOTCONNECTED &&
              session == CO_E_OBJNOTCONNECTED && fixture.keys == before &&
              source.Shutdown() == E_PENDING && IsWindow(source.hwnd()) &&
              source.last_stop() == spike::StopReason::RetiredReferencesPending);
  if (store) store->Release();
  Finish(host, source, 18);
}

void StopCase(HWND root, HINSTANCE instance, Host& host, Mode mode,
              spike::StopReason reason, unsigned test) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = test;
  fixture.mode = mode;
  if (!Start(root, instance, host, source, fixture)) {
    Check(test, false);
    host.source = nullptr;
    return;
  }
  const bool queued = QueueKey(source.hwnd(), false);
  Check(test, queued && Drain(root, host) && !source.accepting() &&
              source.last_stop() == reason && fixture.revokes == 1 &&
              source.counters().tsf_test_down ==
                  (mode == Mode::CancelFails ? 1u : 0u));
  Finish(host, source, test + 1);
}

void PendingCase(HWND root, HINSTANCE instance, Host& host) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = 40;
  fixture.ack = spike::RevokeAck::Pending;
  if (!Start(root, instance, host, source, fixture)) {
    Check(40, false);
    host.source = nullptr;
    return;
  }
  source.BeginRevoke();
  Check(40, !source.accepting() && !source.core_revoked() &&
              source.Shutdown() == E_PENDING && fixture.revokes == 1 &&
              QueueKey(source.hwnd(), false) && Drain(root, host) && fixture.keys == 0);
  // 此确认只模拟父已取得真实撤权回执；不是 Rust 回执证据。
  Check(41, source.ConfirmRevoke() == S_OK && source.core_revoked());
  Finish(host, source, 42);
}

void LedgerCase(HWND root, HINSTANCE instance, Host& host) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = 50;
  fixture.mode = Mode::Busy;
  if (!Start(root, instance, host, source, fixture)) {
    Check(50, false);
    host.source = nullptr;
    return;
  }
  bool queued = true;
  for (unsigned scan = 1; scan <= spike::WindowSource::kRejectionSlots + 1; ++scan) {
    queued = QueueKey(source.hwnd(), false, false, scan) && queued;
  }
  Check(50, queued && Drain(root, host) && !source.accepting() &&
              source.last_stop() == spike::StopReason::RejectionLedgerFull &&
              fixture.revokes == 1 && source.counters().tsf_test_down == 0);
  Finish(host, source, 51);
}

void CapacityCase(HWND root, HINSTANCE instance, Host& host) noexcept {
  SetFocus(root);
  spike::WindowSource sources[5];
  Fixture fixtures[5];
  IUnknown* retained[4]{};
  bool valid = true;
  for (unsigned i = 0; i < 4; ++i) {
    fixtures[i].identity.context_epoch = 60 + i;
    valid = sources[i].Create(root, instance, Options(fixtures[i])) == S_OK && valid;
    valid = sources[i].AcquireTextStoreForProbe(&retained[i]) == S_OK && valid;
    valid = sources[i].Shutdown() == E_PENDING && valid;
  }
  fixtures[4].identity.context_epoch = 64;
  const HRESULT hr = sources[4].Create(root, instance, Options(fixtures[4]));
  Check(60, valid && spike::WindowSource::LiveTextStores() == 4 &&
              hr == HRESULT_FROM_WIN32(ERROR_NOT_ENOUGH_QUOTA) &&
              fixtures[4].revokes == 1);
  for (unsigned i = 0; i < 4; ++i) {
    if (retained[i]) retained[i]->Release();
    valid = sources[i].Shutdown() == S_OK && valid;
  }
  Check(61, valid && spike::WindowSource::LiveTextStores() == 0);
  host.source = nullptr;
}

void RouteCases(HWND root, HINSTANCE instance, Host& host) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = 70;
  if (!Start(root, instance, host, source, fixture)) {
    Check(70, false);
    host.source = nullptr;
    return;
  }
  MSG own{};
  own.hwnd = source.hwnd();
  own.message = WM_CHAR;
  Check(70, source.PreTranslate(own).stop ==
              spike::StopReason::CharacterAssociationUnsupported && fixture.keys == 0);
  own.message = WM_IME_CHAR;
  Check(71, source.PreTranslate(own).stop ==
              spike::StopReason::CharacterAssociationUnsupported && fixture.keys == 0);
  own.message = WM_MOUSEMOVE;
  own.wParam = MK_SHIFT;
  Check(72, source.PreTranslate(own).stop == spike::StopReason::PointerUnsupported &&
              source.counters().pointer_messages_blocked == 1 && fixture.keys == 0);
  own.hwnd = root;
  own.message = WM_KEYDOWN;
  Check(73, source.PreTranslate(own).route == spike::LoopRoute::ExistingRoute &&
              fixture.keys == 0);
  // 故意绕过 loop，证明晚 WndProc 拦截只报接线 STOP，不能当成 pre-IME 成功。
  SendMessageW(source.hwnd(), WM_KEYDOWN, 'A', LPARAM{1} << 16);
  Check(74, source.last_stop() == spike::StopReason::MissingPreTranslate &&
              source.counters().missing_pretranslate == 1 && fixture.keys == 0 &&
              source.counters().tsf_test_down == 0 && fixture.revokes == 1);
  Finish(host, source, 75);
}

void DestructionCase(HWND root, HINSTANCE instance, Host& host) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = 76;
  if (!Start(root, instance, host, source, fixture)) {
    Check(76, false);
    host.source = nullptr;
    return;
  }
  const HWND old = source.hwnd();
  const bool destroyed = DestroyWindow(old) != FALSE;
  Check(76, destroyed && !source.hwnd() && !source.accepting() &&
              source.core_revoked() && fixture.revokes == 1 &&
              source.Shutdown() == S_OK && spike::WindowSource::LiveTextStores() == 0);
  Check(77, source.Create(root, instance, Options(fixture)) == E_UNEXPECTED);
  host.source = nullptr;
}

void RetirementQueueCase(HWND root, HINSTANCE instance, Host& host) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = 80;
  if (!Start(root, instance, host, source, fixture)) {
    Check(80, false);
    host.source = nullptr;
    return;
  }
  bool queued = true;
  for (size_t i = 0; i <= spike::WindowSource::kRetirementDrainSlots; ++i) {
    queued = QueueKey(source.hwnd(), false) && queued;
  }
  source.BeginRevoke();
  Check(80, queued && source.Shutdown() == E_PENDING && IsWindow(source.hwnd()) &&
              source.last_stop() == spike::StopReason::RetiredMessagesPending &&
              !source.accepting() && fixture.keys == 0 && fixture.revokes == 1);
  Check(81, Drain(root, host) && fixture.keys == 0);
  Finish(host, source, 82);
}

void TraceCapacityCase(HWND root, HINSTANCE instance, Host& host) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = 84;
  if (!Start(root, instance, host, source, fixture)) {
    Check(84, false);
    host.source = nullptr;
    return;
  }
  bool queued = true;
  for (unsigned i = 0; i < 100; ++i) {
    queued = QueueKey(source.hwnd(), false, true) && queued;
  }
  Check(84, queued && Drain(root, host) && !source.accepting() &&
              source.trace_size() == spike::WindowSource::kTraceSlots &&
              fixture.revokes == 1 && fixture.keys < 100 &&
              source.counters().tsf_test_down == 0);
  Finish(host, source, 85);
}

void BindReentryCase(HWND root, HINSTANCE instance, Host& host,
                     BindAction action, unsigned test) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.source = &source;
  fixture.identity.context_epoch = test;
  fixture.bind_action = action;
  host.source = &source;
  const HRESULT hr = source.Create(root, instance, Options(fixture));
  std::printf("bind_case=%u create_hr=%08lx accepting=%u revokes=%u nested_shutdown=%08lx\n",
              test, static_cast<unsigned long>(hr), source.accepting() ? 1u : 0u,
              fixture.revokes, static_cast<unsigned long>(fixture.nested_shutdown));
  Check(test, FAILED(hr) && !source.accepting() && fixture.revokes == 1 &&
              source.core_revoked() &&
              (action != BindAction::Revoke || fixture.nested_revoke == spike::RevokeAck::Pending) &&
              (action != BindAction::Deactivate || fixture.nested_deactivate == E_PENDING) &&
              (action != BindAction::Shutdown || fixture.nested_shutdown == E_PENDING));
  Finish(host, source, test + 1);
}

void ShutdownReentryCase(HWND root, HINSTANCE instance, Host& host,
                         ShutdownAction action, unsigned test,
                         bool window_entry = false) noexcept {
  spike::WindowSource source;
  Fixture fixture;
  fixture.identity.context_epoch = test;
  if (action == ShutdownAction::Validate || action == ShutdownAction::Cancel) {
    fixture.mode = Mode::Allow;
  }
  fixture.shutdown_action = action;
  if (!Start(root, instance, host, source, fixture)) {
    Check(test, false);
    host.source = nullptr;
    return;
  }
  bool ran = true;
  if (action == ShutdownAction::Revoke) {
    if (window_entry) SendMessageW(source.hwnd(), WM_KILLFOCUS, 0, 0);
    else source.BeginRevoke();
  } else {
    ran = QueueKey(source.hwnd(), false) && Drain(root, host);
  }
  std::printf("shutdown_case=%u nested_hr=%08lx revokes=%u\n", test,
              static_cast<unsigned long>(fixture.nested_shutdown), fixture.revokes);
  // Source 始终保留在栈上；只观察旧实现错误授予的销毁许可，不执行非法 delete。
  Check(test, ran && fixture.shutdown_called && fixture.nested_shutdown == E_PENDING &&
              !source.accepting() && fixture.revokes == 1);
  Finish(host, source, test + 1);
}

enum class SinkAction { None, InstallOnIdentity, InstallOnSink, StopOnIdentity,
                        StopOnSink, UnadviseOnIdentity, InstallOnRelease,
                        RetireOnIdentity, RetireOnSink, RetireOnRelease };
struct SinkCounts {
  LONG references = 1;
  unsigned adds = 0;
  unsigned releases = 0;
  bool alive = true;
};

class SyntheticSink final : public ITextStoreACPSink {
 public:
  explicit SyntheticSink(SinkCounts& counts) noexcept : counts_(counts) {}
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** result) override {
    if (!result) return E_POINTER;
    *result = nullptr;
    if (iid != IID_IUnknown && iid != __uuidof(ITextStoreACPSink)) return E_NOINTERFACE;
    *result = static_cast<ITextStoreACPSink*>(this);
    AddRef();
    if ((iid == IID_IUnknown && (action == SinkAction::InstallOnIdentity ||
          action == SinkAction::StopOnIdentity || action == SinkAction::UnadviseOnIdentity ||
          action == SinkAction::RetireOnIdentity)) ||
        (iid == __uuidof(ITextStoreACPSink) && (action == SinkAction::InstallOnSink ||
          action == SinkAction::StopOnSink || action == SinkAction::RetireOnSink))) Invoke();
    return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override {
    ++counts_.adds;
    return static_cast<ULONG>(++counts_.references);
  }
  ULONG STDMETHODCALLTYPE Release() override {
    ++counts_.releases;
    const LONG count = --counts_.references;
    if (action == SinkAction::InstallOnRelease && count == 2) Invoke();
    if (action == SinkAction::RetireOnRelease && count == release_trigger) Invoke();
    if (!count) delete this;
    return static_cast<ULONG>(count);
  }
  HRESULT STDMETHODCALLTYPE OnTextChange(DWORD, const TS_TEXTCHANGE*) override { return E_NOTIMPL; }
  HRESULT STDMETHODCALLTYPE OnSelectionChange() override { return E_NOTIMPL; }
  HRESULT STDMETHODCALLTYPE OnLayoutChange(TsLayoutCode, TsViewCookie) override { return E_NOTIMPL; }
  HRESULT STDMETHODCALLTYPE OnStatusChange(DWORD) override { return E_NOTIMPL; }
  HRESULT STDMETHODCALLTYPE OnAttrsChange(LONG, LONG, ULONG, const TS_ATTRID*) override { return E_NOTIMPL; }
  HRESULT STDMETHODCALLTYPE OnLockGranted(DWORD) override { return E_NOTIMPL; }
  HRESULT STDMETHODCALLTYPE OnStartEditTransaction() override { return E_NOTIMPL; }
  HRESULT STDMETHODCALLTYPE OnEndEditTransaction() override { return E_NOTIMPL; }

  SinkAction action = SinkAction::None;
  spike::TsfProbe* probe = nullptr;
  ITextStoreACP* store = nullptr;
  SyntheticSink* other = nullptr;
  HRESULT nested = E_UNEXPECTED;
  ITextStoreACP** client_reference = nullptr;
  LONG release_trigger = 0;
  unsigned client_releases = 0;
  ULONG store_refs_in_foreign_call = 0;
  bool retire_observed = false;
  bool can_release_in_foreign_call = true;

 private:
  ~SyntheticSink() { counts_.alive = false; }
  void Invoke() noexcept {
    const SinkAction selected = action;
    action = SinkAction::None;
    if (selected == SinkAction::StopOnIdentity || selected == SinkAction::StopOnSink) {
      probe->StopAdmissions();
      nested = S_OK;
    } else if (selected == SinkAction::UnadviseOnIdentity) {
      nested = store->UnadviseSink(this);
    } else if (selected == SinkAction::RetireOnIdentity ||
               selected == SinkAction::RetireOnSink ||
               selected == SinkAction::RetireOnRelease) {
      // root 始终由测试栈保留；仅移交并确切释放一次 client 引用，不删除在途对象。
      if (client_reference && *client_reference) {
        ITextStoreACP* client = *client_reference;
        *client_reference = nullptr;
        ++client_releases;
        client->Release();
      }
      nested = probe->Retire();
      can_release_in_foreign_call = probe->CanRelease();
      store_refs_in_foreign_call = store->AddRef() - 1;
      store->Release();
      retire_observed = true;
    } else {
      nested = store->AdviseSink(__uuidof(ITextStoreACPSink), other, TS_AS_TEXT_CHANGE);
    }
  }
  SinkCounts& counts_;
};

void SinkReentryCase(HWND root, SinkAction action, unsigned test) noexcept {
  spike::TsfProbe probe;
  IUnknown* unknown = nullptr;
  ITextStoreACP* store = nullptr;
  HRESULT hr = probe.InitializeStoreForProbe(root, {71, test});
  if (SUCCEEDED(hr)) hr = probe.AcquireStore(&unknown);
  if (SUCCEEDED(hr)) hr = unknown->QueryInterface(IID_PPV_ARGS(&store));
  if (unknown) unknown->Release();
  SinkCounts a_counts;
  SinkCounts b_counts;
  auto* a = new (std::nothrow) SyntheticSink(a_counts);
  auto* b = new (std::nothrow) SyntheticSink(b_counts);
  if (FAILED(hr) || !store || !a || !b) {
    Check(test, false);
    if (a) a->Release();
    if (b) b->Release();
    if (store) store->Release();
    return;
  }
  a->probe = &probe;
  a->store = store;
  a->other = b;
  if (action == SinkAction::InstallOnRelease || action == SinkAction::UnadviseOnIdentity) {
    Check(test, store->AdviseSink(__uuidof(ITextStoreACPSink), a, TS_AS_TEXT_CHANGE) == S_OK);
    a->action = action;
    hr = action == SinkAction::InstallOnRelease ? store->UnadviseSink(a)
        : store->AdviseSink(__uuidof(ITextStoreACPSink), a, TS_AS_SEL_CHANGE);
  } else {
    a->action = action;
    hr = store->AdviseSink(__uuidof(ITextStoreACPSink), a, TS_AS_TEXT_CHANGE);
  }
  std::printf("sink_case=%u outer_hr=%08lx nested_hr=%08lx a_refs=%ld b_refs=%ld\n",
              test, static_cast<unsigned long>(hr), static_cast<unsigned long>(a->nested),
              a_counts.references, b_counts.references);
  const bool installed_b = action == SinkAction::InstallOnIdentity ||
      action == SinkAction::InstallOnSink || action == SinkAction::InstallOnRelease;
  Check(test + 1, (action == SinkAction::InstallOnRelease ? hr == S_OK : FAILED(hr)) &&
      a_counts.references == 1 && b_counts.references == (installed_b ? 3 : 1) &&
      (!installed_b || a->nested == S_OK));
  if (installed_b) {
    Check(test + 2, store->AdviseSink(__uuidof(ITextStoreACPSink), b,
          TS_AS_SEL_CHANGE) == S_OK && b_counts.references == 3);
  }
  store->UnadviseSink(a);
  store->UnadviseSink(b);
  Check(test + 3, a_counts.references == 1 && b_counts.references == 1);
  // 失败断言后的 fixture 回收；每个合成对象最多回收四个额外引用，不改变断言结果。
  for (unsigned i = 0; i < 4 && a_counts.references > 1; ++i) a->Release();
  for (unsigned i = 0; i < 4 && b_counts.references > 1; ++i) b->Release();
  a->Release();
  b->Release();
  Check(test + 4, !a_counts.alive && !b_counts.alive &&
      a_counts.references == 0 && b_counts.references == 0 &&
      a_counts.adds + 1 == a_counts.releases && b_counts.adds + 1 == b_counts.releases);
  store->Release();
}

enum class AcpCall { Advise, AdviseSameIdentity, Unadvise };

void AcpLifetimeCase(HWND root, AcpCall call, SinkAction action,
                     LONG release_trigger, unsigned test) noexcept {
  SinkCounts counts;
  bool association_released = false;
  {
    spike::TsfProbe probe;
    IUnknown* unknown = nullptr;
    ITextStoreACP* client = nullptr;
    HRESULT hr = probe.InitializeStoreForProbe(root, {71, test});
    if (SUCCEEDED(hr)) hr = probe.AcquireStore(&unknown);
    if (SUCCEEDED(hr)) hr = unknown->QueryInterface(IID_PPV_ARGS(&client));
    if (unknown) unknown->Release();
    auto* sink = new (std::nothrow) SyntheticSink(counts);
    if (FAILED(hr) || !client || !sink) {
      Check(test, false);
      if (sink) sink->Release();
      if (client) client->Release();
      return;
    }
    // borrowed 不拥有引用；client 归零后仅依靠此作用域保留的 root 存活。
    ITextStoreACP* const borrowed = client;
    const ULONG initial_refs = borrowed->AddRef() - 1;
    borrowed->Release();
    if (call != AcpCall::Advise) {
      hr = borrowed->AdviseSink(__uuidof(ITextStoreACPSink), sink, TS_AS_TEXT_CHANGE);
    }
    Check(test, hr == S_OK && initial_refs == 2 && !probe.CanRelease());
    sink->probe = &probe;
    sink->store = borrowed;
    sink->client_reference = &client;
    sink->release_trigger = release_trigger;
    sink->action = action;
    hr = call == AcpCall::Unadvise ? borrowed->UnadviseSink(sink)
        : borrowed->AdviseSink(__uuidof(ITextStoreACPSink), sink, TS_AS_SEL_CHANGE);
    std::printf("acp_lifetime_case=%u outer_hr=%08lx retire_hr=%08lx client_releases=%u "
                "in_call_store_refs=%lu in_call_can_release=%u\n", test,
                static_cast<unsigned long>(hr), static_cast<unsigned long>(sink->nested),
                sink->client_releases, sink->store_refs_in_foreign_call,
                sink->can_release_in_foreign_call ? 1u : 0u);
    Check(test + 1, sink->retire_observed && sink->client_releases == 1 && !client &&
        sink->nested == S_OK && !sink->can_release_in_foreign_call &&
        sink->store_refs_in_foreign_call == 2 &&
        hr == (call == AcpCall::Unadvise && action == SinkAction::RetireOnRelease
                   ? S_OK : CO_E_OBJNOTCONNECTED));
    const ULONG returned_refs = borrowed->AddRef() - 1;
    borrowed->Release();
    Check(test + 2, probe.CanRelease() && returned_refs == 1);
    // 外层 ACP 方法已返回后才清理停止的关联与 root；不重复释放 borrowed。
    sink->action = SinkAction::None;
    borrowed->UnadviseSink(sink);
    association_released = counts.references == 1;
    sink->Release();
    if (client) client->Release();
  }
  Check(test + 3, association_released && !counts.alive && counts.references == 0 &&
        counts.adds + 1 == counts.releases && spike::TsfProbe::LiveStores() == 0);
}

void ReviewCases(HWND root, HINSTANCE instance, Host& host) noexcept {
  BindReentryCase(root, instance, host, BindAction::Revoke, 100);
  BindReentryCase(root, instance, host, BindAction::Deactivate, 102);
  BindReentryCase(root, instance, host, BindAction::Destroy, 104);
  BindReentryCase(root, instance, host, BindAction::Shutdown, 106);
  BindReentryCase(root, instance, host, BindAction::KillFocus, 108);
  ShutdownReentryCase(root, instance, host, ShutdownAction::Key, 110);
  ShutdownReentryCase(root, instance, host, ShutdownAction::Validate, 112);
  ShutdownReentryCase(root, instance, host, ShutdownAction::Cancel, 114);
  ShutdownReentryCase(root, instance, host, ShutdownAction::Revoke, 116);
  ShutdownReentryCase(root, instance, host, ShutdownAction::Revoke, 118, true);
  SinkReentryCase(root, SinkAction::InstallOnIdentity, 120);
  SinkReentryCase(root, SinkAction::InstallOnSink, 130);
  SinkReentryCase(root, SinkAction::StopOnIdentity, 140);
  SinkReentryCase(root, SinkAction::StopOnSink, 150);
  SinkReentryCase(root, SinkAction::InstallOnRelease, 160);
  SinkReentryCase(root, SinkAction::UnadviseOnIdentity, 170);
  AcpLifetimeCase(root, AcpCall::Advise, SinkAction::RetireOnIdentity, 0, 180);
  AcpLifetimeCase(root, AcpCall::Advise, SinkAction::RetireOnSink, 0, 184);
  AcpLifetimeCase(root, AcpCall::Unadvise, SinkAction::RetireOnIdentity, 0, 188);
  AcpLifetimeCase(root, AcpCall::AdviseSameIdentity, SinkAction::RetireOnRelease, 3, 192);
  AcpLifetimeCase(root, AcpCall::Unadvise, SinkAction::RetireOnRelease, 1, 196);
}

}  // namespace

int wmain(int argc, wchar_t* argv[]) {
  // 仅接受固定测试 selector，不读取用户配置、凭据、剪贴板或其他窗口内容。
  const bool review_only = argc == 2 && wcscmp(argv[1], L"--review-findings") == 0;
  const bool synthetic_all = argc == 2 && wcscmp(argv[1], L"--synthetic-all") == 0;
  if (argc != 1 && !review_only && !synthetic_all) return 2;
  spike::WindowSource disabled;
  spike::SourceOptions options;
  Check(1, disabled.Create(nullptr, nullptr, options) == S_FALSE &&
             !disabled.hwnd() && !disabled.accepting() &&
             spike::WindowSource::LiveTextStores() == 0);
  MSG synthetic{};
  synthetic.message = WM_KEYDOWN;
  Check(2, disabled.PreTranslate(synthetic).route == spike::LoopRoute::ExistingRoute &&
             disabled.counters().gate == 0);

  const HRESULT com = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  std::printf("com_hr=%08lx source_bytes=%zu\n", static_cast<unsigned long>(com),
              sizeof(spike::WindowSource));
  std::printf("compiler_msc_full_ver=%d pointer_bits=%zu\n", _MSC_FULL_VER,
              sizeof(void*) * 8);
  if (FAILED(com)) return 2;
  USEROBJECTFLAGS flags{};
  DWORD needed = 0;
  const bool interactive = GetUserObjectInformationW(GetProcessWindowStation(),
      UOI_FLAGS, &flags, sizeof(flags), &needed) && (flags.dwFlags & WSF_VISIBLE);
  if (!interactive) {
    std::printf("STOP=noninteractive_session physical=UNVERIFIED ime=UNVERIFIED\n");
    CoUninitialize();
    return 2;
  }
  const HINSTANCE instance = GetModuleHandleW(nullptr);
  Host host;
  WNDCLASSEXW window_class{};
  window_class.cbSize = sizeof(window_class);
  window_class.hInstance = instance;
  window_class.lpfnWndProc = HostProc;
  window_class.lpszClassName = L"GuoSSHell.WaveA.StandaloneFixture";
  const ATOM atom = RegisterClassExW(&window_class);
  HWND root = atom ? CreateWindowExW(0, window_class.lpszClassName,
      L"GuoSSHell Wave A 固定探针", WS_OVERLAPPEDWINDOW,
      CW_USEDEFAULT, CW_USEDEFAULT, 660, 230, nullptr, nullptr, instance, &host)
      : nullptr;
  if (!root) {
    std::printf("STOP=window_creation hr=%08lx\n",
                static_cast<unsigned long>(HRESULT_FROM_WIN32(GetLastError())));
    if (atom) UnregisterClassW(window_class.lpszClassName, instance);
    CoUninitialize();
    return 2;
  }
  const bool synthetic_selector = review_only || synthetic_all;
  ShowWindow(root, synthetic_selector ? SW_SHOWNOACTIVATE : SW_SHOWNORMAL);
  UpdateWindow(root);
  if (!synthetic_selector) SetForegroundWindow(root);
  const bool own_foreground = !synthetic_selector && GetForegroundWindow() == root;
  if (!own_foreground && !review_only && !synthetic_all) {
    std::printf("STOP=run_owned_foreground_unavailable physical=UNVERIFIED ime=UNVERIFIED\n");
    DestroyWindow(root);
    UnregisterClassW(window_class.lpszClassName, instance);
    CoUninitialize();
    return 2;
  }
  if (review_only || synthetic_all) {
    std::printf("synthetic_selector=%u foreground=%s full_foreground_acceptance=UNVERIFIED\n",
                review_only ? 1u : 2u,
                own_foreground ? "RUN_OWNED" : "UNVERIFIED");
  }
  if (!review_only) {
    BasicCases(root, instance, host);
    StopCase(root, instance, host, Mode::WrongContext, spike::StopReason::InvalidTicket, 20);
    StopCase(root, instance, host, Mode::UnknownRaw, spike::StopReason::UnknownRaw, 24);
    StopCase(root, instance, host, Mode::CancelFails, spike::StopReason::TicketCancelPending, 28);
    StopCase(root, instance, host, Mode::ReentrantRevoke, spike::StopReason::Revoked, 32);
    PendingCase(root, instance, host);
    LedgerCase(root, instance, host);
    CapacityCase(root, instance, host);
    RouteCases(root, instance, host);
    DestructionCase(root, instance, host);
    RetirementQueueCase(root, instance, host);
    TraceCapacityCase(root, instance, host);
  }
  ReviewCases(root, instance, host);
  KillTimer(root, kTimeout);
  const bool destroyed = DestroyWindow(root) != FALSE;
  const bool unregistered = UnregisterClassW(window_class.lpszClassName, instance) != FALSE;
  Check(90, destroyed && unregistered && spike::WindowSource::LiveTextStores() == 0);
  CoUninitialize();
  std::printf("summary_checks=%u summary_failures=%d synthetic_os_queue=1 real_rust=UNVERIFIED "
              "physical=UNVERIFIED non_us=UNVERIFIED ime=UNSUPPORTED pointer=UNSUPPORTED\n",
              checks, failures);
  return failures ? 1 : 0;
}
