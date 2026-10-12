#include "native_input_spike_bridge.h"
#include "../native_input_spike_sdk/bootstrap_host.h"

#include <array>
#include <atomic>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <new>

namespace bridge = guosshell::native_input_spike_bridge;
namespace abi = guosshell::native_input_spike_abi;
namespace source = guosshell::native_input_spike;
namespace sdk = guosshell::native_input_spike_sdk;

namespace {
std::atomic<unsigned> checks{0};
std::atomic<unsigned> failures{0};

bool Check(unsigned number, bool condition) noexcept {
  ++checks;
  if (!condition) ++failures;
  std::printf("case=%u result=%u\n", number, condition ? 1U : 0U);
  return condition;
}

struct Observations {
  std::array<bridge::NumericObservation, 256> rows{};
  std::size_t count = 0;
  bool overflow = false;
};

void Observe(void* user, const bridge::NumericObservation& row) noexcept {
  auto& observations = *static_cast<Observations*>(user);
  if (observations.count == observations.rows.size()) observations.overflow = true;
  else observations.rows[observations.count++] = row;
}

struct Spy {
  abi::SpikeOpen request{};
  abi::SpikeOwner cap{};
  abi::SpikeOwner last_cap{};
  abi::SpikeKey last_key{};
  unsigned opens = 0;
  unsigned keys = 0;
  unsigned revokes = 0;
  unsigned supports = 0;
  std::uint64_t seq = 1;
  std::uint32_t status = abi::kOk;
  std::uint32_t disposition = 2;
  std::uint32_t action = 0;
  bool early_error = false;
  bool invalid_reply = false;
};

// 仅单个测试 UI 线程注入的 ABI spy；不复制 Rust policy 或 encoder。
Spy* active_spy = nullptr;

std::uint32_t __cdecl SpyOpen(const abi::SpikeOpen* request,
                              abi::SpikeOwner* out) {
  auto& spy = *active_spy;
  ++spy.opens;
  spy.request = *request;
  spy.cap = {91, request->native_context, 42, 0, 7, {1, 2, 3, 4}};
  *out = spy.cap;
  return abi::kOk;
}

std::uint32_t __cdecl SpyKey(const abi::SpikeOwner* cap,
                             const abi::SpikeKey* key,
                             abi::SpikeAdmission* out) {
  auto& spy = *active_spy;
  ++spy.keys;
  spy.last_cap = *cap;
  spy.last_key = *key;
  if (spy.early_error) return spy.status;
  const auto disposition = key->raw_source == abi::kRawUnknown
      ? (key->phase == abi::kDown ? 0U : 2U) : spy.disposition;
  *out = {abi::kVersion, sizeof(*out), spy.status,
          spy.status == abi::kOk ? disposition : 0U,
          spy.status == abi::kOk ? spy.seq++ : 0,
          spy.status == abi::kOk ? spy.action : 0, 0};
  if (spy.invalid_reply) out->version = 1;
  return spy.status;
}

std::uint32_t __cdecl SpyRevoke(const abi::SpikeOwner* cap) {
  ++active_spy->revokes;
  return std::memcmp(cap, &active_spy->cap, sizeof(*cap)) == 0
             ? abi::kOk : abi::kInvalidSource;
}

std::uint32_t __cdecl SpyJoin(const abi::SpikeOwner*) { return abi::kOk; }
std::uint32_t __cdecl SpyInspect(const abi::SpikeOwner*, abi::SpikeSnapshot*) {
  return abi::kDisabled;
}
std::uint32_t __cdecl SpyMode(const abi::SpikeOwner*, std::uint32_t) {
  return abi::kUnsupported;
}
std::uint32_t __cdecl SpyRead(const abi::SpikeOwner*, std::uint8_t*, std::size_t,
                              std::size_t*) { return abi::kUnsupported; }
std::uint32_t __cdecl SpySupport(const abi::SpikeOwner*, std::uint32_t operation) {
  ++active_spy->supports;
  return operation >= 1 && operation <= 4 ? abi::kUnsupported : abi::kInvalid;
}

bridge::FunctionsTable SpyTable() noexcept {
  return {SpyOpen, SpyKey, SpyRevoke, SpyJoin, SpyInspect, SpyMode, SpyRead,
          SpySupport, bridge::TableEvidence::InjectedSpy};
}

source::NativeKeyEvent Event(source::ContextIdentity context, WPARAM vkey,
    source::KeyPhase phase = source::KeyPhase::Down, unsigned scan = 0x3e,
    bool extended = false) noexcept {
  source::NativeKeyEvent event{};
  event.context = context;
  event.native_event_id = 1;
  event.message = phase == source::KeyPhase::Up ? WM_KEYUP : WM_KEYDOWN;
  event.virtual_key = vkey;
  event.phase = phase;
  event.scan_code = static_cast<std::uint16_t>(scan);
  event.extended = extended;
  std::uint64_t bits = 1 | (std::uint64_t{scan} << 16);
  if (extended) bits |= std::uint64_t{1} << 24;
  if (phase != source::KeyPhase::Down) bits |= std::uint64_t{1} << 30;
  if (phase == source::KeyPhase::Up) bits |= std::uint64_t{1} << 31;
  event.key_bits = static_cast<LPARAM>(bits);
  event.keyboard_state_available = true;
  return event;
}

source::SourceOptions Options(bridge::NativeInputBridge& adapter) noexcept {
  return {true, true, adapter.callbacks()};
}

struct InitializationProbe {
  HANDLE before_init = nullptr;
  HANDLE release_init = nullptr;
  HANDLE checkpoint = nullptr;
  HANDLE release_terminal = nullptr;
  std::atomic<unsigned> publication{0};  // 0 未同步，1 ready，2 thread exit。
  std::atomic<unsigned> unpublished_root_attempts{0};
  std::atomic<unsigned> terminal_posts{0};
  std::atomic<DWORD> initial_wait{WAIT_FAILED};
  std::atomic<bool> owner_complete{false};
};

struct UiRun {
  bridge::FunctionsTable functions;
  bridge::Retirement retirement;
  abi::SpikeOwner owner{};
  Observations observations;
  HANDLE ready = nullptr;
  HANDLE completed = nullptr;
  DWORD command_timeout = 5000;
  HWND root = nullptr;
  source::ContextIdentity context;
  source::NativeKeyEvent event;
  source::GateReply reply;
  HRESULT created = E_UNEXPECTED;
  std::uint32_t text_support = abi::kDisabled;
  std::uint32_t last_status = abi::kDisabled;
  bool command_ok = false;
  bool has_owner = false;
  bool synthetic_only = true;
  bool postmessage_route = false;
  bool accepting = false;
  std::uint32_t source_stop = 0;
  unsigned blocked_characters = 0;
  unsigned tsf_calls = 0;
  std::atomic<HRESULT> cleanup_status{E_UNEXPECTED};
  // 单个共享块：manager 发布后直到匹配 ack 或 UI 退出，不可覆写或读结果。
  std::atomic<std::uint64_t> issued{0};
  std::atomic<std::uint64_t> acknowledged{0};
  std::uint64_t manager_outstanding = 0;
  bool commands_stopped = false;
  std::atomic<bool> stop_requested{false};
  std::atomic<bool> ui_exited{false};
  // 仅生命周期回归使用；普通 fixture 不持有额外 text-store 引用。
  bool serve_spy = false;
  bool hold_reference = false;
  HANDLE pending_retire = nullptr;
  std::atomic<bool> release_reference{false};
  std::atomic<bool> retirement_published{false};
  std::atomic<bool> source_scope_left{false};
  std::atomic<bool> pending_hwnd_alive{false};
  InitializationProbe* initialization_probe = nullptr;
};

constexpr UINT kUiCommand = WM_APP + 91;
constexpr UINT kUiCleanup = WM_APP + 92;
constexpr WPARAM kKey = 1;
constexpr WPARAM kSupport = 2;
constexpr WPARAM kRetire = 3;

bool PumpOwned(source::WindowSource& window, unsigned candidate) noexcept;
bool Shutdown(source::WindowSource& window, bridge::NativeInputBridge& adapter,
              UiRun* run = nullptr) noexcept;

struct QueueCapture {
  source::NativeKeyEvent event;
  bool changed = false;
};
QueueCapture* queue_capture = nullptr;

source::GateReply CaptureQueueKey(void* user, const source::NativeKeyEvent& event) noexcept {
  queue_capture->event = event;
  // 仅更改此新建测试线程的队列键态，证明 bridge 不读取 callback 时的 later state。
  std::array<BYTE, 256> later{};
  later[VK_CONTROL] = later[VK_LCONTROL] = 0x80;
  queue_capture->changed = SetKeyboardState(later.data()) != FALSE;
  auto& adapter = *static_cast<bridge::NativeInputBridge*>(user);
  const auto callbacks = adapter.callbacks();
  const auto result = callbacks.key(callbacks.user, event);
  auto original = event.queue_keyboard_state;
  queue_capture->changed = SetKeyboardState(original.data()) != FALSE && queue_capture->changed;
  return result;
}

bool Queue(source::WindowSource& window, WPARAM vkey, unsigned scan,
           source::KeyPhase phase, unsigned candidate) noexcept {
  const auto event = Event(window.identity(), vkey, phase, scan);
  return PostMessageW(window.hwnd(), event.message, event.virtual_key, event.key_bits) != FALSE &&
         PumpOwned(window, candidate);
}

void SpyWindowCases(HWND root, HINSTANCE instance) noexcept {
  Spy spy;
  active_spy = &spy;
  source::WindowSource window;
  QueueCapture captured;
  queue_capture = &captured;
  bridge::NativeInputBridge adapter(window, SpyTable(), true);
  auto options = Options(adapter);
  options.callbacks.key = CaptureQueueKey;
  const auto created = window.Create(root, instance, options);
  Check(400, created == S_OK);
  if (created == S_OK) {
    std::array<BYTE, 256> previous{};
    const bool saved = GetKeyboardState(previous.data()) != FALSE;
    std::array<BYTE, 256> known{};
    known[VK_SHIFT] = known[VK_LSHIFT] = 0x80;
    const HWND foreground = GetForegroundWindow();
    SetFocus(window.hwnd());
    Check(401, saved && SetKeyboardState(known.data()) && GetFocus() == window.hwnd() &&
               GetForegroundWindow() == foreground);
    Check(402, Queue(window, VK_F9, 0x43, source::KeyPhase::Down, 0) && captured.changed &&
         captured.event.keyboard_state_available && captured.event.queue_keyboard_state[VK_SHIFT] == 0x80 &&
         spy.last_key.modifiers == abi::kShift && spy.last_key.alt_source == 1);
    const auto physical = spy.last_key.physical;
    Check(403, Queue(window, VK_F9, 0x43, source::KeyPhase::Up, 0) &&
         spy.last_key.physical == physical && spy.last_key.phase == abi::kUp);
    const auto before = spy.keys;
    spy.status = abi::kBusy;
    Check(404, Queue(window, VK_F4, 0x3e, source::KeyPhase::Down, 14));
    spy.status = abi::kOk;
    Check(405, Queue(window, VK_F4, 0x3e, source::KeyPhase::Repeat, 14) && spy.keys == before + 1);
    Check(406, Queue(window, VK_F4, 0x3e, source::KeyPhase::Up, 14) &&
         spy.keys == before + 2 && captured.event.local_rejected_lifecycle &&
         spy.last_key.raw_source == abi::kRawUnknown && spy.last_key.phase == abi::kUp);
    const auto invalid_start = spy.keys;
    Check(407, Queue(window, VK_F4, 0x3e, source::KeyPhase::Down, 17) && spy.last_key.raw_source == 0);
    Check(408, Queue(window, VK_F4, 0x3e, source::KeyPhase::Repeat, 14) && spy.last_key.raw_source == 0);
    Check(409, Queue(window, VK_F4, 0x3e, source::KeyPhase::Up, 14) &&
         spy.keys == invalid_start + 3 && spy.last_key.raw_source == 0);
    const auto key_count = spy.keys;
    const auto character_count = window.counters().character_messages_blocked;
    Check(410, PostMessageW(window.hwnd(), WM_CHAR, 'x', 0) &&
         PostMessageW(window.hwnd(), WM_IME_STARTCOMPOSITION, 0, 0) && PumpOwned(window, 14) &&
         spy.keys == key_count && window.counters().character_messages_blocked == character_count + 2 &&
         window.counters().tsf_test_down == 0 && window.counters().tsf_test_up == 0);
    Check(411, saved && SetKeyboardState(previous.data()) != FALSE);
  }
  Check(412, Shutdown(window, adapter));
  queue_capture = nullptr;
  active_spy = nullptr;
}

LRESULT CALLBACK HostProc(HWND hwnd, UINT message, WPARAM wparam,
                          LPARAM lparam) noexcept {
  // 不抢其他窗口的 foreground；仅本次创建的隐藏 fixture host。
  return DefWindowProcW(hwnd, message, wparam, lparam);
}

bool Shutdown(source::WindowSource& window, bridge::NativeInputBridge& adapter,
              UiRun* run) noexcept {
  const HWND hwnd = window.hwnd();
  const HRESULT status = window.Shutdown();
  if (run) run->cleanup_status.store(status);
  if (status != S_OK || (hwnd && IsWindow(hwnd)) ||
      source::WindowSource::LiveTextStores() != 0) return false;
  if (run && adapter.CopyCapability(&run->owner)) {
    if (!adapter.ExportRetirement(&run->retirement)) return false;
    run->has_owner = true;
    run->retirement_published.store(true);
  }
  return true;
}

void SpyCases(HWND root, HINSTANCE instance) noexcept {
  Spy spy;
  active_spy = &spy;
  {
    source::WindowSource window;
    bridge::NativeInputBridge unavailable(window, {}, true);
    Check(1, FAILED(window.Create(root, instance, Options(unavailable))) &&
                 spy.opens == 0 && Shutdown(window, unavailable));
  }
  {
    source::WindowSource window;
    auto table = SpyTable();
    table.evidence = bridge::TableEvidence::Unavailable;
    bridge::NativeInputBridge unavailable(window, table, true);
    Check(2, FAILED(window.Create(root, instance, Options(unavailable))) &&
                 spy.opens == 0 && Shutdown(window, unavailable));
  }
  {
    source::WindowSource window;
    bridge::NativeInputBridge off(window, SpyTable(), false);
    Check(3, FAILED(window.Create(root, instance, Options(off))) &&
                 spy.opens == 0 && Shutdown(window, off));
  }
  {
    source::WindowSource window;
    auto table = SpyTable();
    table.key = nullptr;
    bridge::NativeInputBridge incomplete(window, table, true);
    Check(9, FAILED(window.Create(root, instance, Options(incomplete))) &&
                 spy.opens == 0 && Shutdown(window, incomplete));
  }
  {
    source::WindowSource window;
    bridge::NativeInputBridge wrong(window, SpyTable(), true);
    source::ContextIdentity identity;
    auto callbacks = wrong.callbacks();
    Check(4, callbacks.bind(callbacks.user, root, &identity) ==
                 source::Admission::InvalidSource && spy.opens == 0);
  }
  source::WindowSource window;
  Observations observations;
  bridge::NativeInputBridge adapter(window, SpyTable(), true, {&observations, Observe});
  const HRESULT create = window.Create(root, instance, Options(adapter));
  Check(5, create == S_OK);
  if (create != S_OK) {
    Check(6, Shutdown(window, adapter));
    return;
  }
  Check(7, spy.request.version == abi::kVersion && spy.request.size == sizeof(spy.request) &&
               spy.request.explicit_opt_in == 1 && spy.request.reserved == 0 &&
               spy.request.native_context == &window &&
               spy.request.fixture_name_len == sizeof(abi::kFixtureName) - 1 &&
               std::memcmp(spy.request.fixture_name, abi::kFixtureName,
                           spy.request.fixture_name_len) == 0);
  auto callbacks = adapter.callbacks();
  const auto context = window.identity();
  const WPARAM keys[] = {VK_RETURN, VK_ESCAPE, VK_TAB, VK_BACK, VK_DELETE, VK_INSERT,
      VK_HOME, VK_END, VK_PRIOR, VK_NEXT, VK_UP, VK_DOWN, VK_LEFT, VK_RIGHT};
  const std::uint32_t kinds[] = {2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15};
  for (std::size_t i = 0; i < std::size(keys); ++i) {
    const auto reply = callbacks.key(callbacks.user, Event(context, keys[i]));
    Check(20 + static_cast<unsigned>(i), reply.raw_proof == source::RawProof::Trusted &&
        spy.last_key.raw_kind == kinds[i] && spy.last_key.raw_value == 0 &&
        spy.last_key.code_kind == kinds[i] && spy.last_key.legacy_kind == kinds[i] &&
        spy.last_key.dual_source == 1 && spy.last_key.text_ownership == abi::kTerminalKey);
  }
  for (unsigned i = 0; i < 24; ++i) {
    const auto reply = callbacks.key(callbacks.user, Event(context, VK_F1 + i));
    Check(40 + i, reply.raw_proof == source::RawProof::Trusted &&
        spy.last_key.raw_kind == abi::kFunction && spy.last_key.raw_value == i + 1 &&
        spy.last_key.code_value == i + 1 && spy.last_key.legacy_value == i + 1);
  }
  for (unsigned i = 0; i < 3; ++i) {
    const auto phase = static_cast<source::KeyPhase>(i);
    callbacks.key(callbacks.user, Event(context, VK_LEFT, phase, 0x4b, true));
    Check(70 + i, spy.last_key.phase == i &&
        spy.last_key.physical == (bridge::NativeInputBridge::kImmutableNativeDomain | 0x14b));
  }
  auto modified = Event(context, VK_F8);
  for (const unsigned key : {VK_SHIFT, VK_LSHIFT, VK_CONTROL, VK_RCONTROL,
                            VK_MENU, VK_LMENU, VK_RWIN}) {
    modified.queue_keyboard_state[key] = 0x80;
  }
  callbacks.key(callbacks.user, modified);
  Check(73, spy.last_key.modifiers == 15 && spy.last_key.alt_source == 1 &&
            spy.last_key.alt_sides == 1 && spy.last_key.raw_source == abi::kRawVerified);
  const WPARAM unknowns[] = {'A', VK_OEM_7, VK_PROCESSKEY, VK_PACKET, 0, 0x10000};
  for (unsigned i = 0; i < std::size(unknowns); ++i) {
    const auto reply = callbacks.key(callbacks.user, Event(context, unknowns[i]));
    Check(80 + i, reply.raw_proof == source::RawProof::Unknown &&
        reply.disposition == source::Disposition::Rejected && spy.last_key.raw_source == 0 &&
        spy.last_key.raw_kind == 0 && spy.last_key.dual_source == 0 &&
        spy.last_key.code_kind == 0 && spy.last_key.legacy_kind == 0 &&
        spy.last_key.text_ownership == abi::kTextUnverified && reply.text.ticket == 0);
    callbacks.key(callbacks.user, Event(context, VK_F4, source::KeyPhase::Up));
  }
  for (unsigned i = 0; i < 7; ++i) {
    auto event = Event(context, VK_F4);
    if (i == 0) event.keyboard_state_available = false;
    if (i == 1) event.queue_keyboard_state[VK_RMENU] = 0x80;
    if (i == 2) event.queue_keyboard_state[VK_CONTROL] = 0x80;
    if (i == 3) event.default_candidate_intent = 17;
    if (i == 4) event.extended = true;
    if (i == 5) event.message = WM_CHAR;
    if (i == 6) event.local_rejected_lifecycle = true;
    callbacks.key(callbacks.user, event);
    Check(90 + i, spy.last_key.raw_source == abi::kRawUnknown &&
        spy.last_key.dual_source == 0 && spy.last_key.modifiers == 0 &&
        spy.last_key.default_action <= 16 && !adapter.revoked());
    callbacks.key(callbacks.user, Event(context, VK_F4, source::KeyPhase::Up));
  }
  for (unsigned i = 0; i <= 16; ++i) {
    auto event = Event(context, VK_F9);
    event.default_candidate_intent = i;
    callbacks.key(callbacks.user, event);
    Check(100 + i, spy.last_key.default_action == i);
  }
  auto stale = Event(context, VK_F2);
  ++stale.context.context_epoch;
  const auto before = spy.keys;
  Check(120, callbacks.key(callbacks.user, stale).admission == source::Admission::InvalidSource &&
       spy.keys == before && callbacks.revoke(callbacks.user, stale.context) == source::RevokeAck::Failed);
  ++stale.context.owner_cookie;
  Check(121, callbacks.key(callbacks.user, stale).admission == source::Admission::InvalidSource &&
       spy.keys == before && !adapter.revoked());
  Check(122, adapter.TextSupport() == abi::kUnsupported &&
       !callbacks.validate_text(callbacks.user, modified, {}) &&
       !callbacks.cancel_text(callbacks.user, {}) && adapter.last_status() == abi::kUnsupported);
  const std::uint32_t statuses[] = {abi::kBusy, abi::kStaleOwner, abi::kInvalid,
      abi::kInvalidSource, abi::kUnsupported, abi::kDisabled, abi::kStopped};
  const source::Admission expected[] = {source::Admission::Busy, source::Admission::StaleOwner,
      source::Admission::InvalidSource, source::Admission::InvalidSource,
      source::Admission::Unsupported, source::Admission::Unsupported, source::Admission::Unsupported};
  for (unsigned i = 0; i < std::size(statuses); ++i) {
    spy.status = statuses[i];
    const auto reply = callbacks.key(callbacks.user, Event(context, VK_F9));
    Check(130 + i, reply.admission == expected[i] &&
                  reply.disposition == source::Disposition::Rejected);
    callbacks.key(callbacks.user, Event(context, VK_F9, source::KeyPhase::Up));
  }
  spy.status = abi::kStaleOwner;
  spy.early_error = true;
  Check(137, callbacks.key(callbacks.user, Event(context, VK_F9)).admission ==
             source::Admission::StaleOwner);
  spy.status = abi::kInvalidSource;
  Check(138, callbacks.key(callbacks.user, Event(context, VK_F9)).admission ==
             source::Admission::InvalidSource);
  spy.early_error = false;
  spy.status = abi::kOk;
  spy.disposition = 1;
  Check(140, callbacks.key(callbacks.user, Event(context, VK_F9)).disposition ==
             source::Disposition::Encoded);
  spy.disposition = 4;
  for (unsigned i = 1; i <= 16; ++i) {
    spy.action = i;
    callbacks.key(callbacks.user, Event(context, VK_F9));
    Check(150 + i, observations.rows[observations.count - 1].result.action == i &&
         observations.rows[observations.count - 1].result.seq != 0);
  }
  abi::SpikeOwner copied{};
  Check(170, adapter.CopyCapability(&copied) &&
       std::memcmp(&copied, &spy.cap, sizeof(copied)) == 0 &&
       std::memcmp(&spy.last_cap, &spy.cap, sizeof(copied)) == 0);
  bridge::Retirement retirement;
  Check(171, !adapter.ExportRetirement(&retirement));
  // 故意注入 nonDown action：不能再次交付观察 callback，更不能放行文本。
  const auto observed = observations.count;
  const auto reply = callbacks.key(callbacks.user,
                                  Event(context, VK_F9, source::KeyPhase::Repeat));
  Check(172, reply.admission == source::Admission::Unsupported && adapter.revoked() &&
             observations.count == observed);
  Check(173, adapter.ExportRetirement(&retirement) &&
             retirement.JoinOnManagementThread() == abi::kInvalidSource);
  const auto keys_before_revoke = spy.keys;
  Check(174, callbacks.key(callbacks.user, Event(context, VK_F4)).admission ==
             source::Admission::StaleOwner && spy.keys == keys_before_revoke);
  Check(175, callbacks.bind(callbacks.user, window.hwnd(), &stale.context) ==
             source::Admission::StaleOwner && spy.opens == 1);
  Check(176, Shutdown(window, adapter) && !observations.overflow);
  // 无 scan：不调用 key、不伪造 physical=1，真实撤权保持一次绑定。
  Spy missing_scan;
  active_spy = &missing_scan;
  {
    source::WindowSource other;
    bridge::NativeInputBridge no_scan(other, SpyTable(), true);
    const auto hr = other.Create(root, instance, Options(no_scan));
    Check(180, hr == S_OK);
    if (hr == S_OK) {
      auto cb = no_scan.callbacks();
      const auto result = cb.key(cb.user, Event(other.identity(), VK_F4,
                                              source::KeyPhase::Down, 0));
      Check(181, result.raw_proof == source::RawProof::Unknown && no_scan.revoked() &&
           missing_scan.keys == 0 && missing_scan.revokes == 1);
    }
    Check(182, Shutdown(other, no_scan));
  }
  Spy invalid_result;
  active_spy = &invalid_result;
  {
    source::WindowSource other;
    Observations rejected;
    bridge::NativeInputBridge invalid(other, SpyTable(), true, {&rejected, Observe});
    const auto hr = other.Create(root, instance, Options(invalid));
    Check(183, hr == S_OK);
    if (hr == S_OK) {
      invalid_result.invalid_reply = true;
      const auto cb = invalid.callbacks();
      const auto result = cb.key(cb.user, Event(other.identity(), VK_F4));
      Check(184, invalid.revoked() && result.admission == source::Admission::Unsupported &&
                 result.text.ticket == 0 && rejected.count == 0);
    }
    Check(185, Shutdown(other, invalid));
  }
  Spy full_ledger;
  active_spy = &full_ledger;
  {
    source::WindowSource other;
    bridge::NativeInputBridge bounded(other, SpyTable(), true);
    const auto hr = other.Create(root, instance, Options(bounded));
    Check(186, hr == S_OK);
    if (hr == S_OK) {
      const auto cb = bounded.callbacks();
      for (unsigned scan = 1; scan <= source::WindowSource::kRejectionSlots + 1; ++scan) {
        cb.key(cb.user, Event(other.identity(), 'A', source::KeyPhase::Down, scan));
      }
      Check(187, bounded.revoked() && bounded.last_status() == abi::kExhausted);
    }
    Check(188, Shutdown(other, bounded));
  }
  active_spy = nullptr;
  SpyWindowCases(root, instance);
  std::printf("layer=1 checks=%u failures=%u\n", checks.load(), failures.load());
}

bool Inspect(const bridge::FunctionsTable& functions, const abi::SpikeOwner& owner,
             abi::SpikeSnapshot* snapshot) noexcept {
  const auto status = functions.inspect(&owner, snapshot);
  return status == abi::kOk && snapshot->status == status &&
         snapshot->version == abi::kVersion && snapshot->size == sizeof(*snapshot);
}

bool WaitSnapshot(const bridge::FunctionsTable& functions, const abi::SpikeOwner& owner,
                  abi::SpikeSnapshot* snapshot) noexcept {
  const ULONGLONG deadline = GetTickCount64() + 3000;
  do {
    const auto status = functions.inspect(&owner, snapshot);
    if (status != abi::kBusy) {
      return status == abi::kOk && snapshot->status == status &&
          snapshot->version == abi::kVersion && snapshot->size == sizeof(*snapshot);
    }
    // 只等待非 UI 诊断查询可读，不重试输入或重放任何已准入动作。
    Sleep(1);
  } while (GetTickCount64() < deadline);
  return false;
}

// 以下等待只属于独立测试的输出观测，不在 bridge／输入 callback 中。
bool WaitIdle(const bridge::FunctionsTable& functions,
              const abi::SpikeOwner& owner, std::uint64_t remote) noexcept {
  const ULONGLONG deadline = GetTickCount64() + 3000;
  do {
    abi::SpikeSnapshot snapshot{};
    if (Inspect(functions, owner, &snapshot) && snapshot.remote_applied >= remote &&
        snapshot.charged_slots == 0 && snapshot.inflight == 0) return true;
    Sleep(1);
  } while (GetTickCount64() < deadline);
  return false;
}

bool ReadBytes(const bridge::FunctionsTable& functions, const abi::SpikeOwner& owner,
    const std::uint8_t* expected, std::size_t length) noexcept {
  std::array<std::uint8_t, 128> bytes{};
  std::size_t total = 0;
  const ULONGLONG deadline = GetTickCount64() + 3000;
  do {
    std::size_t count = 0;
    const auto status = functions.peer_read(&owner, bytes.data() + total,
                                           bytes.size() - total, &count);
    if (status != abi::kOk && status != abi::kBusy) return false;
    if (count > bytes.size() - total) return false;
    total += count;
    if (total >= length) return total == length &&
        std::memcmp(bytes.data(), expected, length) == 0;
    Sleep(1);
  } while (GetTickCount64() < deadline);
  return false;
}

bool PumpOwned(source::WindowSource& window, unsigned candidate) noexcept {
  // 只读本次 source 的消息队列；绝不观察任意其他 HWND 的按键。
  MSG message{};
  unsigned processed = 0;
  while (processed < 32 && PeekMessageW(&message, window.hwnd(), 0, 0, PM_REMOVE)) {
    ++processed;
    const auto decision = window.PreTranslate(message, candidate);
    if (decision.route == source::LoopRoute::ExistingRoute) return false;
    if (decision.route == source::LoopRoute::DispatchOnly) DispatchMessageW(&message);
  }
  return processed != 0;
}

void ServeActualUi(HWND root, HINSTANCE instance, UiRun& run) noexcept {
  source::WindowSource window;
  bridge::NativeInputBridge adapter(window, run.functions, true, {&run.observations, Observe});
  IUnknown* retained = nullptr;
  run.created = window.Create(root, instance, Options(adapter));
  run.root = root;
  run.context = window.identity();
  adapter.CopyCapability(&run.owner);
  bool retiring = run.created != S_OK;
  if (run.created == S_OK && run.hold_reference) {
    Check(520, window.AcquireTextStoreForProbe(&retained) == S_OK && retained);
  }
  // 只设置新测试 UI 线程自有 HWND 的 focus，绝不 SetForegroundWindow。
  const HWND foreground = GetForegroundWindow();
  if (!retiring) SetFocus(window.hwnd());
  run.postmessage_route = GetFocus() == window.hwnd() && window.accepting();
  run.synthetic_only = true;  // PostMessage 不是物理按键；hidden foreground 也不补证明。
  Check(200, GetForegroundWindow() == foreground);
  SetEvent(run.ready);
  std::uint64_t retire_command = 0;
  const auto finish_retirement = [&]() noexcept {
    if (retained && run.release_reference.load()) {
      retained->Release();
      retained = nullptr;
    }
    run.command_ok = Shutdown(window, adapter, &run);
    if (!run.command_ok) {
      run.pending_hwnd_alive.store(window.hwnd() && IsWindow(window.hwnd()));
      if (run.pending_retire) SetEvent(run.pending_retire);
      return false;
    }
    run.blocked_characters = window.counters().character_messages_blocked;
    run.tsf_calls = window.counters().tsf_test_down + window.counters().tsf_down +
                    window.counters().tsf_test_up + window.counters().tsf_up;
    if (retire_command) {
      run.acknowledged.store(retire_command, std::memory_order_release);
      SetEvent(run.completed);
    }
    return true;
  };
  if (retiring && finish_retirement()) return;
  MSG message{};
  for (;;) {
    const BOOL received = GetMessageW(&message, nullptr, 0, 0);
    if (received <= 0) {
      Check(305, false);
      retiring = true;
      run.stop_requested.store(true);
    }
    if (retiring || run.stop_requested.load(std::memory_order_acquire)) {
      retiring = true;
      if (finish_retirement()) return;
      // Pending 保留 source/bridge/sink/root；下一正常消息入口继续，UI 不等 worker。
      continue;
    }
    if (message.hwnd == root && message.message == kUiCleanup) {
      retiring = true;
      if (finish_retirement()) return;
      continue;
    }
    if (message.hwnd != root || message.message != kUiCommand) {
      // 输入只从本测试 root 和 source 的消息到达，不观察其他窗口内容。
      if (message.hwnd == window.hwnd()) {
        const auto decision = window.PreTranslate(message);
        if (decision.route == source::LoopRoute::DispatchOnly) DispatchMessageW(&message);
      } else if (message.hwnd == root) DispatchMessageW(&message);
      continue;
    }
    const auto command_id = static_cast<std::uint64_t>(message.lParam);
    if (!command_id || run.issued.load(std::memory_order_acquire) != command_id ||
        run.acknowledged.load() >= command_id) {
      Check(306, false);
      retiring = true;
      run.stop_requested.store(true);
      if (finish_retirement()) return;
      continue;
    }
    run.command_ok = true;
    if (message.wParam == kKey) {
      if (run.postmessage_route) {
        run.command_ok = window.accepting() && GetFocus() == window.hwnd() &&
            PostMessageW(window.hwnd(), run.event.message,
            run.event.virtual_key, run.event.key_bits) != FALSE &&
            PumpOwned(window, run.event.default_candidate_intent);
      } else {
        const auto callbacks = adapter.callbacks();
        run.reply = callbacks.key(callbacks.user, run.event);
      }
      run.last_status = adapter.last_status();
      run.accepting = window.accepting();
      run.source_stop = static_cast<std::uint32_t>(window.last_stop());
    } else if (message.wParam == kSupport) {
      run.text_support = adapter.TextSupport();
      const auto callbacks = adapter.callbacks();
      run.command_ok = !callbacks.validate_text(callbacks.user, run.event, {}) &&
                       !callbacks.cancel_text(callbacks.user, {});
    } else if (message.wParam == kRetire) {
      window.BeginRevoke();
      const auto callbacks = adapter.callbacks();
      run.reply = callbacks.key(callbacks.user, Event(run.context, VK_F4));
      retiring = true;
      retire_command = command_id;
      if (finish_retirement()) return;
      continue;
    } else run.command_ok = false;
    run.acknowledged.store(command_id, std::memory_order_release);
    SetEvent(run.completed);
  }
}

bool CanSend(const UiRun& run) noexcept {
  return !run.commands_stopped && !run.manager_outstanding && !run.ui_exited.load();
}

void StopCommands(UiRun& run) noexcept {
  run.commands_stopped = true;
  run.stop_requested.store(true, std::memory_order_release);
  // 只发终结唤醒；不携带共享 payload，也不消费迟到 completed 作为新命令 ack。
  PostMessageW(run.root, kUiCleanup, 0, 0);
}

bool UiCommand(UiRun& run, WPARAM command) noexcept {
  if (!CanSend(run)) return false;
  const auto command_id = run.issued.load() + 1;
  run.manager_outstanding = command_id;
  run.issued.store(command_id, std::memory_order_release);
  const bool posted = PostMessageW(run.root, kUiCommand, command,
                                  static_cast<LPARAM>(command_id)) != FALSE;
  const auto waited = posted ? WaitForSingleObject(run.completed, run.command_timeout) : WAIT_FAILED;
  if (!posted || waited != WAIT_OBJECT_0 ||
      run.acknowledged.load(std::memory_order_acquire) != command_id) {
    std::printf("command_id=%llu wait=%lu posted=%u ownership_returned=0\n",
                command_id, waited, posted ? 1U : 0U);
    StopCommands(run);
    return false;
  }
  run.manager_outstanding = 0;
  return run.command_ok;
}

bool Send(UiRun& run, WPARAM vkey, unsigned scan, source::KeyPhase phase,
           unsigned candidate = 14) noexcept {
  if (!CanSend(run)) return false;
  run.event = Event(run.context, vkey, phase, scan);
  run.event.default_candidate_intent = candidate;
  return UiCommand(run, kKey);
}

// Busy 是要验证的结果，不是同一 Down 的重试：先验证 Repeat 不复活、完成 Up，
// 才允许下一个独立 physical press。所有 output 等待都在管理 caller。
bool BeginPress(UiRun& run, WPARAM vkey, unsigned scan) noexcept {
  if (!CanSend(run)) return false;
  for (unsigned press = 0; press < 8; ++press) {
    const auto before = run.observations.count;
    if (!Send(run, vkey, scan, source::KeyPhase::Down) ||
        run.observations.count != before + 1) return false;
    const auto& down = run.observations.rows[before];
    if (down.result.status == abi::kOk) return true;
    if (down.result.status != abi::kBusy || down.result.seq || down.result.action ||
        down.result.byte_len) return false;
    if (!WaitIdle(run.functions, run.owner, 0) ||
        !Send(run, vkey, scan, source::KeyPhase::Repeat) ||
        !Send(run, vkey, scan, source::KeyPhase::Up)) return false;
    for (std::size_t i = before; i < run.observations.count; ++i) {
      const auto& row = run.observations.rows[i];
      if (row.result.action || row.result.byte_len) return false;
    }
    const auto& up = run.observations.rows[run.observations.count - 1];
    if (up.phase != abi::kUp || up.raw_source != abi::kRawUnknown ||
        up.result.status != abi::kInvalid) return false;
    std::printf("busy_press_retired=1 press=%u\n", press);
  }
  return false;
}

enum class PressureOutcome { Verified, PreconditionMissing, ProtocolFailure };

PressureOutcome PressurePremise(std::size_t effective_bytes, bool pending_write) noexcept {
  if (effective_bytes <= 128) return PressureOutcome::PreconditionMissing;
  return pending_write ? PressureOutcome::Verified : PressureOutcome::ProtocolFailure;
}

bool EmptyPressureReply(const abi::SpikeAdmission& reply) noexcept {
  return !reply.disposition && !reply.seq && !reply.action && !reply.byte_len;
}

bool ConfirmPendingF7Up(const abi::SpikeSnapshot& snapshot, std::uint64_t* next,
                        std::size_t* effective, bool* deferred) noexcept {
  if (*deferred && snapshot.next_seq == *next + 1) {
    ++*next;
    *effective += 9;
    *deferred = false;
  }
  return snapshot.next_seq == *next;
}

void BusyScheduleNeedsAdmittedPressure() noexcept {
  std::size_t effective = 0;
  unsigned accepted = 0;
  unsigned busy = 0;
  // 注入数字轨迹不是 actual Rust；16 次独立 press 中 8 次 Busy、8 次真实形状准入。
  for (unsigned press = 0; press < 16; ++press) {
    const abi::SpikeAdmission down{abi::kVersion, sizeof(abi::SpikeAdmission),
        press % 2 ? abi::kOk : abi::kBusy, press % 2 ? 1U : 0U,
        press % 2 ? std::uint64_t{accepted * 2 + 1} : 0, 0, press % 2 ? 5U : 0U};
    if (down.status == abi::kBusy) { ++busy; continue; }
    const abi::SpikeAdmission up{abi::kVersion, sizeof(abi::SpikeAdmission), abi::kOk,
        1, down.seq + 1, 0, 9};
    effective += down.byte_len + up.byte_len;
    ++accepted;
  }
  Check(550, busy == 8 && accepted == 8 && effective == 112);
  Check(551, PressurePremise(effective, false) == PressureOutcome::PreconditionMissing);
  Check(552, PressurePremise(140, true) == PressureOutcome::Verified &&
             PressurePremise(140, false) != PressureOutcome::Verified);
  const abi::SpikeAdmission rejected{abi::kVersion, sizeof(abi::SpikeAdmission), abi::kBusy, 0, 0, 0, 0};
  auto release = rejected;
  release.status = abi::kInvalid;
  Check(553, EmptyPressureReply(rejected) && EmptyPressureReply(release) &&
             release.status == abi::kInvalid);
  std::uint64_t next = 18;  // Accepted Down 的 seq17／5 bytes 已存在。
  std::size_t pending_effective = 5;
  bool deferred = true;
  abi::SpikeSnapshot before{};
  before.next_seq = next;
  Check(554, ConfirmPendingF7Up(before, &next, &pending_effective, &deferred) &&
             deferred && pending_effective == 5);  // Up Busy 的 seq0 不是完成或 9 bytes。
  before.next_seq = 19;
  Check(555, ConfirmPendingF7Up(before, &next, &pending_effective, &deferred) &&
             !deferred && pending_effective == 14 && next == 19);
  Check(556, ConfirmPendingF7Up(before, &next, &pending_effective, &deferred) && pending_effective == 14);
  before.next_seq = 20;
  Check(557, !ConfirmPendingF7Up(before, &next, &pending_effective, &deferred));
  std::printf("pressure_spy=1 busy=%u accepted=%u effective_bytes=%zu outcome=%u actual_rust=0\n",
      busy, accepted, effective, static_cast<unsigned>(PressurePremise(effective, false)));
}

void PressureSnapshot(unsigned stage, unsigned press, const abi::SpikeSnapshot& snapshot,
                      std::size_t effective, std::size_t read) noexcept {
  std::printf("pressure_stage=%u press=%u status=%u next=%llu last=%llu slots=%u charge=%u inflight=%u stopped=%u done=%u joined=%u effective=%zu read=%zu\n",
      stage, press, snapshot.status, snapshot.next_seq, snapshot.last_written_seq,
      snapshot.charged_slots, snapshot.charged_bytes, snapshot.inflight, snapshot.stopped,
      snapshot.worker_done, snapshot.worker_joined, effective, read);
}

bool PressureEvent(UiRun& run, unsigned press, source::KeyPhase phase,
                   bridge::NumericObservation* row, bool* present) noexcept {
  const auto before = run.observations.count;
  if (!Send(run, VK_F7, 0x41, phase, 0)) return false;
  *present = run.observations.count == before + 1;
  if (run.observations.count != before && !*present) return false;
  if (*present) {
    *row = run.observations.rows[before];
    std::printf("pressure_event=%u phase=%u present=1 raw=%u modifiers=%u status=%u disposition=%u seq=%llu bytes=%u action=%u\n",
        press, static_cast<unsigned>(phase), row->raw_source, row->modifiers,
        row->result.status, row->result.disposition, row->result.seq,
        row->result.byte_len, row->result.action);
  } else std::printf("pressure_event=%u phase=%u present=0 local_block=1\n", press,
                     static_cast<unsigned>(phase));
  return true;
}

bool PausedPeerCharge(UiRun& run) noexcept {
  constexpr std::uint8_t down_bytes[] = {0x1b, '[', '1', '8', '~'};
  constexpr std::uint8_t up_bytes[] = {0x1b, '[', '1', '8', ';', '1', ':', '3', '~'};
  std::array<std::uint8_t, 16 * (sizeof(down_bytes) + sizeof(up_bytes))> expected{};
  std::size_t expected_size = 0;
  std::size_t effective = 0;
  abi::SpikeSnapshot baseline{};
  if (!WaitSnapshot(run.functions, run.owner, &baseline) || baseline.charged_slots ||
      baseline.inflight || baseline.stopped || baseline.worker_done || baseline.worker_joined) return false;
  PressureSnapshot(0, 0, baseline, effective, 0);
  auto next = baseline.next_seq;
  const ULONGLONG admission_deadline = GetTickCount64() + 3000;
  unsigned presses = 0;
  unsigned busy_down = 0;
  unsigned pending_up = 0;
  bool deferred_up = false;
  abi::SpikeSnapshot held{};
  const auto snapshot_until = [&](abi::SpikeSnapshot* snapshot) noexcept {
    do {
      if (Inspect(run.functions, run.owner, snapshot)) {
        return snapshot->hub_instance == baseline.hub_instance &&
            snapshot->engine_instance == baseline.engine_instance &&
            snapshot->remote_applied == baseline.remote_applied &&
            !snapshot->stopped && !snapshot->worker_done && !snapshot->worker_joined &&
            snapshot->charged_slots <= 4 && snapshot->charged_bytes <= 128 &&
            snapshot->charged_bytes == snapshot->charged_slots * 32;
      }
      Sleep(1);
    } while (GetTickCount64() < admission_deadline);
    return false;
  };
  for (; presses < 16 && effective <= 128 && GetTickCount64() < admission_deadline; ++presses) {
    if (!CanSend(run)) return false;
    abi::SpikeSnapshot before{};
    if (!snapshot_until(&before) || before.next_seq != next) return false;
    PressureSnapshot(1, presses, before, effective, 0);
    bridge::NumericObservation down{};
    bool present = false;
    if (!PressureEvent(run, presses, source::KeyPhase::Down, &down, &present) || !present) return false;
    if (down.result.status == abi::kBusy) {
      if (!EmptyPressureReply(down.result)) return false;
      ++busy_down;
      bridge::NumericObservation repeat{};
      if (!PressureEvent(run, presses, source::KeyPhase::Repeat, &repeat, &present) ||
          (present && (repeat.result.byte_len || repeat.result.action))) return false;
      bridge::NumericObservation up{};
      if (!PressureEvent(run, presses, source::KeyPhase::Up, &up, &present) || !present ||
          up.raw_source != abi::kRawUnknown || up.result.status != abi::kInvalid ||
          up.result.seq || up.result.byte_len || up.result.action) return false;
      abi::SpikeSnapshot rejected{};
      if (!snapshot_until(&rejected) || rejected.next_seq != next) return false;
      PressureSnapshot(2, presses, rejected, effective, 0);
      continue;  // 原 physical 已 Up，下一次才是独立 press；不重试同一 Down。
    }
    if (down.result.status != abi::kOk || down.result.disposition != 1 || down.result.seq != next ||
        down.result.byte_len != sizeof(down_bytes) || down.result.action || down.modifiers ||
        down.raw_source != abi::kRawVerified) return false;
    ++next;
    effective += down.result.byte_len;
    std::memcpy(expected.data() + expected_size, down_bytes, sizeof(down_bytes));
    expected_size += sizeof(down_bytes);
    bridge::NumericObservation up{};
    if (!PressureEvent(run, presses, source::KeyPhase::Up, &up, &present) || !present) return false;
    if (up.result.status == abi::kOk) {
      if (up.result.disposition != 1 || up.result.seq != next || up.result.byte_len != sizeof(up_bytes) ||
          up.result.action) return false;
      ++next;
      effective += up.result.byte_len;
    } else if (up.result.status == abi::kBusy && EmptyPressureReply(up.result)) {
      ++pending_up;
      deferred_up = true;  // 尚未计 9 bytes；不补发 Up。
    } else return false;
    std::memcpy(expected.data() + expected_size, up_bytes, sizeof(up_bytes));
    expected_size += sizeof(up_bytes);
    bool progressed = false;
    do {
      abi::SpikeSnapshot after{};
      if (!snapshot_until(&after)) return false;
      // 单 producer/无 remote 的新增 seq，最终 peer literal 再核验。
      if (!ConfirmPendingF7Up(after, &next, &effective, &deferred_up)) return false;
      if (effective > 128 && after.inflight == 1 && after.charged_slots) {
        held = after;
        progressed = true;
      } else if (effective <= 128 && !deferred_up && after.last_written_seq == next - 1 &&
                 !after.charged_slots && !after.inflight) progressed = true;
      if (progressed) { PressureSnapshot(3, presses, after, effective, 0); break; }
      Sleep(1);
    } while (GetTickCount64() < admission_deadline);
    if (!progressed) {
      std::printf("pressure_precondition_failed=1 presses=%u effective=%zu deferred_up=%u\n",
          presses + 1, effective, deferred_up ? 1U : 0U);
      return false;
    }
  }
  const bool pending_write = held.inflight == 1 && held.charged_slots != 0;
  const auto premise = PressurePremise(effective, pending_write);
  std::printf("pressure_premise=%u presses=%u busy_down=%u pending_up=%u effective=%zu expected=%zu no_peer_read=1\n",
      static_cast<unsigned>(premise), presses, busy_down, pending_up, effective, expected_size);
  if (premise != PressureOutcome::Verified) return false;
  abi::SpikeSnapshot still_held{};
  if (!snapshot_until(&still_held) || still_held.last_written_seq != held.last_written_seq ||
      still_held.inflight != 1 || still_held.charged_slots != held.charged_slots ||
      still_held.charged_bytes != held.charged_bytes) return false;
  PressureSnapshot(4, presses, still_held, effective, 0);
  std::array<std::uint8_t, 128> bytes{};
  std::size_t total_read = 0;
  const ULONGLONG read_deadline = GetTickCount64() + 3000;
  do {
    std::size_t count = 0;
    const auto status = run.functions.peer_read(&run.owner, bytes.data(), bytes.size(), &count);
    std::printf("pressure_read_status=%u offset=%zu count=%zu\n", status, total_read, count);
    if ((status != abi::kOk && status != abi::kBusy) || count > bytes.size() ||
        count > expected_size - total_read ||
        std::memcmp(bytes.data(), expected.data() + total_read, count) != 0) return false;
    total_read += count;
    abi::SpikeSnapshot after{};
    if (Inspect(run.functions, run.owner, &after)) {
      PressureSnapshot(5, presses, after, effective, total_read);
      if (after.hub_instance != baseline.hub_instance || after.engine_instance != baseline.engine_instance ||
          after.remote_applied != baseline.remote_applied || after.stopped || after.worker_done ||
          after.worker_joined || after.charged_bytes > 128 || after.charged_slots > 4) return false;
      if (!ConfirmPendingF7Up(after, &next, &effective, &deferred_up)) return false;
      if (status == abi::kBusy && !count && !deferred_up && total_read == expected_size &&
          effective == expected_size && !after.charged_slots && !after.charged_bytes &&
          !after.inflight && after.last_written_seq == next - 1) {
        std::printf("pressure_fullwrite=1 literal_bytes=%zu duplicate_bytes=0 busy_down=%u pending_up=%u\n",
            total_read, busy_down, pending_up);
        return true;
      }
    }
    Sleep(1);
  } while (GetTickCount64() < read_deadline);
  std::printf("pressure_read_failed=1 effective=%zu expected=%zu actual=%zu deferred_up=%u\n",
      effective, expected_size, total_read, deferred_up ? 1U : 0U);
  return false;
}

void ActualCases(UiRun& run) noexcept {
  Check(201, run.created == S_OK);
  if (run.created != S_OK) return;
  const auto& owner = run.owner;
  Check(202, owner.native_context && owner.handle == run.context.owner_cookie &&
             owner.generation == run.context.context_epoch);
  abi::SpikeSnapshot initial{};
  if (!Check(203, WaitSnapshot(run.functions, owner, &initial) && initial.hub_instance &&
             initial.engine_instance && initial.owners == 1 && initial.remote_applied == 0)) return;
  constexpr std::uint8_t f4[] = {0x1b, '[', '3', '~'};
  constexpr std::uint8_t f5[] = {0x7f};
  bool f4_started = BeginPress(run, VK_F4, 0x3e);
  if (!f4_started) { Check(204, false); return; }
  bool f4_read = f4_started && ReadBytes(run.functions, owner, f4, sizeof(f4));
  if (!f4_read) { Check(204, false); return; }
  bool f4_up = Send(run, VK_F4, 0x3e, source::KeyPhase::Up);
  if (!Check(204, f4_started && f4_read && f4_up && WaitIdle(run.functions, owner, 0))) return;
  bool f5_started = BeginPress(run, VK_F5, 0x3f);
  if (!f5_started) { Check(205, false); return; }
  bool f5_read = f5_started && ReadBytes(run.functions, owner, f5, sizeof(f5));
  if (!f5_read) { Check(205, false); return; }
  bool f5_up = Send(run, VK_F5, 0x3f, source::KeyPhase::Up);
  if (!Check(205, f5_started && f5_read && f5_up && WaitIdle(run.functions, owner, 0))) return;
  const auto start = run.observations.count;
  const bool f2_down = BeginPress(run, VK_F2, 0x3c);
  if (!f2_down) { Check(206, false); return; }
  const bool f2_repeat = Send(run, VK_F2, 0x3c, source::KeyPhase::Repeat);
  if (!f2_repeat) { Check(206, false); return; }
  const bool f2_up = Send(run, VK_F2, 0x3c, source::KeyPhase::Up);
  if (!Check(206, f2_down && f2_repeat && f2_up && WaitIdle(run.functions, owner, 0))) return;
  unsigned actions = 0;
  for (std::size_t i = start; i < run.observations.count; ++i) {
    const auto& row = run.observations.rows[i];
    if (row.result.action == 1 && row.phase == abi::kDown) ++actions;
    Check(210 + static_cast<unsigned>(i - start), row.result.action == 0 ||
                                                  (row.phase == abi::kDown && row.result.action == 1));
  }
  Check(213, actions == 1 && !run.observations.overflow);
  if (!Check(214, UiCommand(run, kSupport) && run.text_support == abi::kUnsupported &&
             run.functions.support(&owner, 1) == abi::kUnsupported &&
             run.functions.support(&owner, 3) == abi::kUnsupported)) return;
  // 模式从真实 stream peer 写入，由 engine 回应 query；不缓存 mode 或复制 encoder。
  for (std::uint32_t command = 1; command <= 3; ++command) {
    Check(220 + command, run.functions.peer_mode(&owner, command) == abi::kOk &&
                        WaitIdle(run.functions, owner, (command - 1) * 2 + 1));
    Check(230 + command, run.functions.peer_mode(&owner, 0) == abi::kOk);
    const std::uint8_t response[] = {0x1b, '[', '?', static_cast<std::uint8_t>('0' + command), 'u'};
    Check(240 + command, ReadBytes(run.functions, owner, response, sizeof(response)) &&
                        WaitIdle(run.functions, owner, command * 2));
  }
  if (!Check(249, PausedPeerCharge(run))) return;
  // Unknown Down 的同一 physical 在随后可信 Repeat／Up 不能复活绑定／候选。
  const auto unknown_start = run.observations.count;
  if (!Check(250, Send(run, 'A', 0x1e, source::KeyPhase::Down))) return;
  const bool unknown_repeat = Send(run, VK_F2, 0x1e, source::KeyPhase::Repeat);
  if (!unknown_repeat) { Check(251, false); return; }
  const bool unknown_up = Send(run, VK_F2, 0x1e, source::KeyPhase::Up);
  if (!unknown_up) { Check(251, false); return; }
  bool rejected_until_up = unknown_repeat && unknown_up &&
      run.observations.count >= unknown_start + 2 && WaitIdle(run.functions, owner, 6);
  for (std::size_t i = unknown_start; i < run.observations.count; ++i) {
    const auto& row = run.observations.rows[i];
    rejected_until_up = rejected_until_up && row.raw_source == abi::kRawUnknown &&
                       row.result.action == 0 && row.result.byte_len == 0;
  }
  Check(251, rejected_until_up);
  auto bad = owner;
  ++bad.generation;
  abi::SpikeAdmission unused{};
  abi::SpikeKey key{};
  Check(252, run.functions.key(&bad, &key, &unused) == abi::kInvalidSource);
  bad = owner;
  bad.handle = 100;
  Check(253, run.functions.revoke(&bad) == abi::kStaleOwner);
  abi::SpikeSnapshot stable{};
  Check(254, WaitSnapshot(run.functions, owner, &stable) &&
       stable.hub_instance == initial.hub_instance && stable.engine_instance == initial.engine_instance);
  if (!Check(255, UiCommand(run, kRetire) && run.has_owner)) return;
  Check(256, run.reply.admission == source::Admission::StaleOwner && run.tsf_calls == 0);
  std::printf("layer=2 synthetic_only=%u postmessage_route=%u owner=%u sdk_same_process=0\n",
              run.synthetic_only ? 1U : 0U, run.postmessage_route ? 1U : 0U,
              run.has_owner ? 1U : 0U);
}

DWORD WINAPI RunUi(void* parameter) noexcept {
  auto& run = *static_cast<UiRun*>(parameter);
  if (run.initialization_probe) {
    SetEvent(run.initialization_probe->before_init);
    WaitForSingleObject(run.initialization_probe->release_init, INFINITE);
  }
  const auto com = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  if (FAILED(com)) {
    Check(300, false);
    run.ui_exited.store(true);
    SetEvent(run.ready);
    return 1;
  }
  const HINSTANCE instance = GetModuleHandleW(nullptr);
  constexpr wchar_t class_name[] = L"GuoSSHell.WaveA.Bridge.Test";
  WNDCLASSEXW window_class{};
  window_class.cbSize = sizeof(window_class);
  window_class.lpfnWndProc = HostProc;
  window_class.hInstance = instance;
  window_class.lpszClassName = class_name;
  const ATOM registered = RegisterClassExW(&window_class);
  const HWND root = registered ? CreateWindowExW(WS_EX_NOACTIVATE, class_name,
      L"固定 bridge fixture", WS_OVERLAPPEDWINDOW, 0, 0, 660, 220,
      nullptr, nullptr, instance, nullptr) : nullptr;
  Check(301, root != nullptr);
  if (root) {
    Spy spy;
    if (run.serve_spy) active_spy = &spy;
    if (run.functions.evidence == bridge::TableEvidence::InjectedSpy && !run.serve_spy) SpyCases(root, instance);
    else {
      ServeActualUi(root, instance, run);
      run.source_scope_left.store(true);
    }
    active_spy = nullptr;
    Check(302, DestroyWindow(root) != FALSE);
  }
  Check(303, !registered || UnregisterClassW(class_name, instance) != FALSE);
  Check(304, source::WindowSource::LiveTextStores() == 0);
  CoUninitialize();
  run.ui_exited.store(true, std::memory_order_release);
  SetEvent(run.ready);
  return failures ? 1U : 0U;
}

bool MappedHere(HMODULE module, FARPROC address) noexcept {
  MEMORY_BASIC_INFORMATION memory{};
  HMODULE containing = nullptr;
  return address && VirtualQuery(reinterpret_cast<const void*>(address), &memory,
                                 sizeof(memory)) == sizeof(memory) &&
      memory.State == MEM_COMMIT && memory.Type == MEM_IMAGE &&
      memory.AllocationBase == module &&
      GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                        reinterpret_cast<LPCWSTR>(address), &containing) && containing == module;
}

bool StandaloneTable(const wchar_t* path, bridge::FunctionsTable* table) {
  sdk::BootstrapHost parent;
  std::array<std::uint8_t, 16> nonce{};
  nonce.fill(7);
  // 仅父现有加载器加载指定既有 fixture；不创建副本，不调用 SDK probe 或伪造 Verify。
  const auto diagnostic = parent.Preflight(path, true, nonce);
  if (diagnostic.status != sdk::Status::kOk || !diagnostic.fixture_only ||
      diagnostic.core_abi != abi::kVersion || diagnostic.pointer_bits != 64 ||
      diagnostic.owners != 0 || diagnostic.core_exports != 8 || diagnostic.rinf_exports != 4) return false;
  // lookup 只选择父已映射的 module；随后 FROM_ADDRESS 检查才是归属证据。
  const HMODULE module = GetModuleHandleW(parent.compiled_lib_path().c_str());
  if (!module) return false;
  const char* names[] = {abi::kOpenSymbol, abi::kKeySymbol, abi::kRevokeSymbol,
      abi::kJoinSymbol, abi::kInspectSymbol, abi::kPeerModeSymbol,
      abi::kPeerReadSymbol, abi::kSupportSymbol};
  std::array<FARPROC, 8> addresses{};
  for (std::size_t i = 0; i < addresses.size(); ++i) {
    addresses[i] = GetProcAddress(module, names[i]);
    if (!MappedHere(module, addresses[i])) return false;
  }
  *table = {reinterpret_cast<abi::OpenFn>(addresses[0]), reinterpret_cast<abi::KeyFn>(addresses[1]),
      reinterpret_cast<abi::RevokeFn>(addresses[2]), reinterpret_cast<abi::JoinFn>(addresses[3]),
      reinterpret_cast<abi::InspectFn>(addresses[4]), reinterpret_cast<abi::PeerModeFn>(addresses[5]),
      reinterpret_cast<abi::PeerReadFn>(addresses[6]), reinterpret_cast<abi::SupportFn>(addresses[7]),
      bridge::TableEvidence::StandaloneCheckedFixture};
  std::printf("preflight_fixture=1 core_abi=2 pointer_bits=64 owners=0 mapped_exports=8\n");
  return true;
}

struct TimeoutProbe {
  UiRun run;
  HANDLE before_read = nullptr;
  HANDLE release_read = nullptr;
  source::KeyPhase observed_phase = source::KeyPhase::Up;
  unsigned posted_commands = 0;
};

DWORD WINAPI PausedCommandUi(void* parameter) noexcept {
  auto& probe = *static_cast<TimeoutProbe*>(parameter);
  const HINSTANCE instance = GetModuleHandleW(nullptr);
  constexpr wchar_t name[] = L"GuoSSHell.WaveA.Bridge.Handoff";
  WNDCLASSEXW window_class{};
  window_class.cbSize = sizeof(window_class);
  window_class.hInstance = instance;
  window_class.lpfnWndProc = HostProc;
  window_class.lpszClassName = name;
  const ATOM registered = RegisterClassExW(&window_class);
  probe.run.root = registered ? CreateWindowExW(WS_EX_NOACTIVATE, name, L"固定交接探针",
      WS_OVERLAPPEDWINDOW, 0, 0, 100, 100, nullptr, nullptr, instance, nullptr) : nullptr;
  SetEvent(probe.run.ready);
  MSG message{};
  if (probe.run.root && GetMessageW(&message, probe.run.root, kUiCommand, kUiCommand) > 0) {
    ++probe.posted_commands;
    // 可控暂停发生在共享 payload 的首次读取之前；不存在并发读写复现。
    SetEvent(probe.before_read);
    WaitForSingleObject(probe.release_read, INFINITE);
    probe.observed_phase = probe.run.event.phase;
    probe.run.command_ok = true;
    probe.run.acknowledged.store(static_cast<std::uint64_t>(message.lParam), std::memory_order_release);
    SetEvent(probe.run.completed);
    while (PeekMessageW(&message, probe.run.root, kUiCommand, kUiCommand, PM_REMOVE)) {
      ++probe.posted_commands;
    }
  }
  if (probe.run.root) Check(503, DestroyWindow(probe.run.root) != FALSE);
  if (registered) Check(504, UnregisterClassW(name, instance) != FALSE);
  probe.run.ui_exited.store(true);
  return 0;
}

void TimeoutDoesNotReusePayload() noexcept {
  TimeoutProbe probe;
  probe.run.command_timeout = 0;
  probe.run.ready = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  probe.run.completed = CreateEventW(nullptr, FALSE, FALSE, nullptr);
  probe.before_read = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  probe.release_read = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  const bool handles = probe.run.ready && probe.run.completed && probe.before_read && probe.release_read;
  const HANDLE ui = handles ? CreateThread(nullptr, 0, PausedCommandUi, &probe, 0, nullptr) : nullptr;
  if (ui) {
    const bool ready = WaitForSingleObject(probe.run.ready, 5000) == WAIT_OBJECT_0;
    const bool first = ready && Send(probe.run, VK_F4, 0x3e, source::KeyPhase::Down);
    const bool paused = WaitForSingleObject(probe.before_read, 5000) == WAIT_OBJECT_0;
    const bool second = paused && Send(probe.run, VK_F5, 0x3f, source::KeyPhase::Up);
    SetEvent(probe.release_read);
    const bool exited = WaitForSingleObject(ui, INFINITE) == WAIT_OBJECT_0;
    Check(500, ready && paused && !first && !second && exited);
    Check(501, probe.observed_phase == source::KeyPhase::Down);
    Check(502, probe.posted_commands == 1);
    Check(506, !Send(probe.run, VK_F6, 0x40, source::KeyPhase::Repeat) &&
               probe.run.issued.load() == 1 && probe.run.acknowledged.load() == 1 &&
               probe.run.manager_outstanding == 1 && probe.run.commands_stopped);
    std::printf("timeout_seen_phase=%u ordinary_posts=%u ui_exited=%u\n",
        static_cast<unsigned>(probe.observed_phase), probe.posted_commands, exited ? 1U : 0U);
    CloseHandle(ui);
  } else Check(500, false);
  for (HANDLE handle : {probe.run.ready, probe.run.completed, probe.before_read, probe.release_read}) {
    if (handle) CloseHandle(handle);
  }
}

DWORD WINAPI PendingShutdownUi(void*) noexcept {
  const auto com = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  if (FAILED(com)) { Check(510, false); return 1; }
  const HINSTANCE instance = GetModuleHandleW(nullptr);
  constexpr wchar_t name[] = L"GuoSSHell.WaveA.Bridge.Pending";
  WNDCLASSEXW window_class{};
  window_class.cbSize = sizeof(window_class);
  window_class.hInstance = instance;
  window_class.lpfnWndProc = HostProc;
  window_class.lpszClassName = name;
  const ATOM registered = RegisterClassExW(&window_class);
  const HWND root = registered ? CreateWindowExW(WS_EX_NOACTIVATE, name, L"固定退休探针",
      WS_OVERLAPPEDWINDOW, 0, 0, 100, 100, nullptr, nullptr, instance, nullptr) : nullptr;
  Check(510, root != nullptr);
  if (root) {
    Spy spy;
    active_spy = &spy;
    source::WindowSource window;
    bridge::NativeInputBridge adapter(window, SpyTable(), true);
    UiRun run;
    IUnknown* retained = nullptr;
    const auto create = window.Create(root, instance, Options(adapter));
    const auto acquired = create == S_OK ? window.AcquireTextStoreForProbe(&retained) : E_FAIL;
    const HWND source_hwnd = window.hwnd();
    Check(511, create == S_OK && acquired == S_OK && retained);
    if (retained) {
      const bool complete = Shutdown(window, adapter, &run);
      Check(512, !complete && run.cleanup_status.load() == E_PENDING && IsWindow(source_hwnd));
      Check(513, !run.has_owner && !run.retirement.join);
      std::printf("pending_hr=%08lx retirement_exported=%u hwnd_alive=%u stores=%ld\n",
          static_cast<unsigned long>(run.cleanup_status.load()), run.has_owner ? 1U : 0U,
          IsWindow(source_hwnd) ? 1U : 0U, source::WindowSource::LiveTextStores());
      retained->Release();
    }
    // 引用释放且 Shutdown S_OK 退栈后，才允许离开对象 scope。
    bool complete = Shutdown(window, adapter, &run);
    Check(514, complete && run.has_owner && run.retirement.join && !IsWindow(source_hwnd));
    if (!complete) std::printf("cleanup_complete=0 scope_retained=1\n");
    // 失败断言不授予销毁许可；仅后续正常 UI 入口能够完成清理。
    while (!complete) {
      MSG message{};
      GetMessageW(&message, nullptr, 0, 0);
      complete = Shutdown(window, adapter, &run);
    }
    active_spy = nullptr;
    Check(515, DestroyWindow(root) != FALSE);
  }
  if (registered) Check(516, UnregisterClassW(name, instance) != FALSE);
  Check(517, source::WindowSource::LiveTextStores() == 0);
  CoUninitialize();
  return 0;
}

void PendingRetainsUiScope(InitializationProbe* probe = nullptr) noexcept {
  auto* owner = new (std::nothrow) UiRun;
  if (!owner) { Check(521, false); return; }
  auto& run = *owner;
  run.initialization_probe = probe;
  run.functions = SpyTable();
  run.serve_spy = true;
  run.hold_reference = true;
  run.command_timeout = 0;
  run.ready = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  run.completed = CreateEventW(nullptr, FALSE, FALSE, nullptr);
  run.pending_retire = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  const bool handles = run.ready && run.completed && run.pending_retire;
  const HANDLE ui = handles ? CreateThread(nullptr, 0, RunUi, &run, 0, nullptr) : nullptr;
  if (ui) {
    const auto initial = WaitForSingleObject(run.ready, probe ? 0 : 5000);
    const bool ready = initial == WAIT_OBJECT_0;
    if (probe) {
      probe->initial_wait.store(initial);
      if (ready) probe->publication.store(1);
    }
    bool exit_confirmed = false;
    if (!ready) {
      if (probe) SetEvent(probe->checkpoint);
      else Check(521, false);
      // 初始化 timeout 不归还共享状态：只等 ready 发布或实际 thread exit。
      const HANDLE publication[] = {run.ready, ui};
      const auto observed = WaitForMultipleObjects(2, publication, FALSE, INFINITE);
      if (observed != WAIT_OBJECT_0 && observed != WAIT_OBJECT_0 + 1) {
        Check(527, false);
        std::printf("init_owner_retained=1 wait=%lu\n", observed);
        return;
      }
      exit_confirmed = observed == WAIT_OBJECT_0 + 1;
      if (probe) probe->publication.store(exit_confirmed ? 2U : 1U);
    }
    const bool retired = ready && UiCommand(run, kRetire);
    const bool pending = ready && WaitForSingleObject(run.pending_retire, 5000) == WAIT_OBJECT_0;
    // 未归还共享块时只读 atomic 诊断；不读 capability、reply 或 observations。
    if (ready) {
      Check(521, ready && !retired && pending && run.cleanup_status.load() == E_PENDING);
      Check(522, !run.retirement_published.load() && !run.source_scope_left.load() &&
                 !run.ui_exited.load() && run.pending_hwnd_alive.load());
      std::printf("pending_scope_left=%u retirement_published=%u ui_exited=%u\n",
          run.source_scope_left.load() ? 1U : 0U, run.retirement_published.load() ? 1U : 0U,
          run.ui_exited.load() ? 1U : 0U);
    }
    run.release_reference.store(true);
    if (probe) {
      if (!probe->publication.load()) ++probe->unpublished_root_attempts;
      SetEvent(probe->checkpoint);
      WaitForSingleObject(probe->release_terminal, INFINITE);
      // 读前 watchpoint 只观察已获得的发布；缺少同步不授予任何 root 访问许可。
      if (!probe->publication.load()) {
        Check(527, false);
        std::printf("init_owner_retained=1 publication=0\n");
        return;
      }
    }
    if (!exit_confirmed) {
      if (probe) ++probe->terminal_posts;
      PostMessageW(run.root, kUiCleanup, 0, 0);
    }
    const bool exited = exit_confirmed || WaitForSingleObject(ui, INFINITE) == WAIT_OBJECT_0;
    if (!exited) {
      Check(523, false);
      std::printf("init_owner_retained=1 ui_exit_confirmed=0\n");
      return;
    }
    Check(523, exited && run.ui_exited.load() && run.source_scope_left.load() &&
               run.cleanup_status.load() == S_OK && run.retirement_published.load());
    Check(524, run.has_owner && run.retirement.JoinOnManagementThread() == abi::kOk);
    Check(525, source::WindowSource::LiveTextStores() == 0);
    CloseHandle(ui);
  } else Check(521, false);
  for (HANDLE handle : {run.ready, run.completed, run.pending_retire}) {
    if (handle) CloseHandle(handle);
  }
  if (probe) probe->owner_complete.store(true);
  delete owner;
}

DWORD WINAPI RunInitializationTimeout(void* parameter) noexcept {
  PendingRetainsUiScope(static_cast<InitializationProbe*>(parameter));
  return 0;
}

void InitializationTimeoutDoesNotReadRoot() noexcept {
  auto* probe = new (std::nothrow) InitializationProbe;
  if (!probe) { Check(530, false); return; }
  probe->before_init = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  probe->release_init = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  probe->checkpoint = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  probe->release_terminal = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  const bool handles = probe->before_init && probe->release_init &&
                       probe->checkpoint && probe->release_terminal;
  const HANDLE manager = handles ? CreateThread(nullptr, 0, RunInitializationTimeout,
                                                probe, 0, nullptr) : nullptr;
  if (manager) {
    const bool paused = WaitForSingleObject(probe->before_init, 5000) == WAIT_OBJECT_0;
    const bool checkpoint = WaitForSingleObject(probe->checkpoint, 5000) == WAIT_OBJECT_0;
    Check(530, paused && checkpoint && probe->initial_wait.load() == WAIT_TIMEOUT);
    Check(531, probe->unpublished_root_attempts.load() == 0 && probe->terminal_posts.load() == 0 &&
               !probe->owner_complete.load() && WaitForSingleObject(manager, 0) == WAIT_TIMEOUT);
    std::printf("init_wait=%lu unpublished_root_attempts=%u terminal_posts=%u owner_complete=%u\n",
        probe->initial_wait.load(), probe->unpublished_root_attempts.load(),
        probe->terminal_posts.load(), probe->owner_complete.load() ? 1U : 0U);
    SetEvent(probe->release_init);
    SetEvent(probe->release_terminal);
    const bool exited = WaitForSingleObject(manager, INFINITE) == WAIT_OBJECT_0;
    Check(532, exited && probe->owner_complete.load() && probe->publication.load() != 0);
    std::printf("init_publication=%u terminal_posts=%u owner_complete=%u manager_exited=%u\n",
        probe->publication.load(), probe->terminal_posts.load(),
        probe->owner_complete.load() ? 1U : 0U, exited ? 1U : 0U);
    if (!exited || !probe->owner_complete.load()) return;  // 保留可能仍被 UI 使用的 owner/handles。
    CloseHandle(manager);
  } else Check(530, false);
  for (HANDLE handle : {probe->before_init, probe->release_init, probe->checkpoint, probe->release_terminal}) {
    if (handle) CloseHandle(handle);
  }
  delete probe;
}

int SafetyCases() noexcept {
  std::printf("fixture_process=%lu layer=3 actual_rust=0\n", GetCurrentProcessId());
  TimeoutDoesNotReusePayload();
  const HANDLE ui = CreateThread(nullptr, 0, PendingShutdownUi, nullptr, 0, nullptr);
  Check(518, ui && WaitForSingleObject(ui, INFINITE) == WAIT_OBJECT_0);
  if (ui) CloseHandle(ui);
  PendingRetainsUiScope();
  InitializationTimeoutDoesNotReadRoot();
  BusyScheduleNeedsAdmittedPressure();
  std::printf("checks=%u failures=%u\n", checks.load(), failures.load());
  return failures ? 1 : 0;
}
}  // namespace

int wmain(int argc, wchar_t** argv) {
  if (argc == 2 && std::wcscmp(argv[1], L"--safety") == 0) return SafetyCases();
  std::printf("fixture_process=%lu\n", GetCurrentProcessId());
  UiRun run;
  if (argc == 2 && std::wcscmp(argv[1], L"--spy") == 0) run.functions = SpyTable();
  else if (argc == 3 && std::wcscmp(argv[1], L"--standalone-fixture") == 0) {
    if (!StandaloneTable(argv[2], &run.functions)) {
      std::printf("standalone_preflight=0\n");
      return 2;
    }
  } else return 2;
  // 主线程是管理 caller；UI 线程自身不等 Rust worker、不 join。
  run.ready = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  run.completed = CreateEventW(nullptr, FALSE, FALSE, nullptr);
  if (!run.ready || !run.completed) {
    if (run.ready) CloseHandle(run.ready);
    if (run.completed) CloseHandle(run.completed);
    return 2;
  }
  const HANDLE ui = CreateThread(nullptr, 0, RunUi, &run, 0, nullptr);
  if (!ui) { CloseHandle(run.ready); CloseHandle(run.completed); return 2; }
  if (run.functions.evidence != bridge::TableEvidence::InjectedSpy) {
    if (WaitForSingleObject(run.ready, 5000) == WAIT_OBJECT_0) ActualCases(run);
    else {
      Check(309, false);
      // UI 初始化尚未完成时不能释放其栈或留下悬空共享状态。
      WaitForSingleObject(run.ready, INFINITE);
      if (run.created == S_OK) UiCommand(run, kRetire);
    }
    if (!run.ui_exited.load()) StopCommands(run);
  }
  const auto waited = WaitForSingleObject(ui, INFINITE);
  CloseHandle(ui);
  CloseHandle(run.ready);
  CloseHandle(run.completed);
  Check(310, waited == WAIT_OBJECT_0);
  if (run.has_owner) {
    Check(311, run.retirement.JoinOnManagementThread() == abi::kOk);
    abi::SpikeSnapshot snapshot{};
    Check(312, Inspect(run.functions, run.owner, &snapshot) && snapshot.stopped == 1 &&
        snapshot.worker_done == 1 && snapshot.worker_joined == 1 &&
        snapshot.charged_slots == 0 && snapshot.charged_bytes == 0 && snapshot.inflight == 0);
    std::printf("worker_joined=%u charged_slots=%u charged_bytes=%u\n",
                snapshot.worker_joined, snapshot.charged_slots, snapshot.charged_bytes);
  }
  std::printf("checks=%u failures=%u\n", checks.load(), failures.load());
  return failures ? 1 : 0;
}
