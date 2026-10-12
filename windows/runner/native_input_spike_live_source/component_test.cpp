#include "source_fixture_host.h"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <utility>

namespace live = guosshell::native_input_spike_live_source;
namespace abi = guosshell::native_input_spike_abi;
namespace source = guosshell::native_input_spike;
namespace bridge = guosshell::native_input_spike_bridge;

namespace {
unsigned checks = 0;
unsigned failures = 0;

bool Check(unsigned number, bool condition) noexcept {
  ++checks;
  if (!condition) ++failures;
  std::printf("case=%u result=%u\n", number, condition ? 1U : 0U);
  return condition;
}

// 只观察宿主层；响应由 case 配置，不实现 Rust binding matcher／encoder。
struct LayerSpy {
  abi::SpikeOwner owner{};
  abi::SpikeKey key{};
  unsigned opens = 0;
  unsigned keys = 0;
  unsigned revokes = 0;
  unsigned revoke_busy = 0;
  std::uint64_t seq = 1;
  std::uint32_t status = abi::kOk;
  std::uint32_t disposition = 2;
  std::uint32_t action = 0;
  std::uint32_t byte_len = 0;
  std::array<std::uint8_t, 4> payload{0x1b, 0x5b, 0x33, 0x7e};
  std::size_t peer_capacity = 0;
  bool peer_oversize_reply = false;
  DWORD ui_thread = 0;
  std::atomic<DWORD> join_thread{0};
  std::atomic<unsigned> joins{0};
  std::atomic<bool> joined{false};
  HANDLE release_join = nullptr;
} spy;

bool SameOwner(const abi::SpikeOwner* owner) noexcept {
  return owner && std::memcmp(owner, &spy.owner, sizeof(*owner)) == 0;
}

std::uint32_t __cdecl Open(const abi::SpikeOpen* request, abi::SpikeOwner* out) {
  ++spy.opens;
  spy.ui_thread = GetCurrentThreadId();
  spy.owner = {57, request->native_context, 12, 0, 3, {1, 2, 3}};
  *out = spy.owner;
  return abi::kOk;
}

std::uint32_t __cdecl Key(const abi::SpikeOwner* owner, const abi::SpikeKey* key,
                           abi::SpikeAdmission* out) {
  if (!SameOwner(owner)) return abi::kInvalidSource;
  ++spy.keys;
  spy.key = *key;
  const bool approved = spy.status == abi::kOk && key->raw_source == abi::kRawVerified;
  *out = {abi::kVersion, sizeof(*out), spy.status,
      approved ? spy.disposition : 0U, spy.status == abi::kOk ? spy.seq++ : 0,
      approved ? spy.action : 0U, approved ? spy.byte_len : 0U};
  return spy.status;
}

std::uint32_t __cdecl Revoke(const abi::SpikeOwner* owner) {
  if (!SameOwner(owner)) return abi::kInvalidSource;
  ++spy.revokes;
  if (spy.revoke_busy) { --spy.revoke_busy; return abi::kBusy; }
  return abi::kOk;
}

std::uint32_t __cdecl Join(const abi::SpikeOwner* owner) {
  if (!SameOwner(owner)) return abi::kInvalidSource;
  spy.join_thread.store(GetCurrentThreadId());
  ++spy.joins;
  // 只有组件专用管理线程在此等待受控测试 barrier，UI 从不等待。
  if (spy.release_join && WaitForSingleObject(spy.release_join, 5000) != WAIT_OBJECT_0) return abi::kBusy;
  spy.joined.store(true);
  return abi::kOk;
}

std::uint32_t __cdecl Inspect(const abi::SpikeOwner* owner, abi::SpikeSnapshot* out) {
  if (!SameOwner(owner)) return abi::kInvalidSource;
  *out = {};
  out->version = abi::kVersion;
  out->size = sizeof(*out);
  out->status = abi::kOk;
  out->owners = 1;
  out->next_seq = spy.seq;
  if (spy.joined.load()) out->stopped = out->worker_done = out->worker_joined = 1;
  return abi::kOk;
}

std::uint32_t __cdecl Mode(const abi::SpikeOwner* owner, std::uint32_t mode) {
  return SameOwner(owner) ? (mode <= 4 ? abi::kOk : abi::kInvalid) : abi::kInvalidSource;
}

std::uint32_t __cdecl Read(const abi::SpikeOwner* owner, std::uint8_t* bytes,
                           std::size_t capacity, std::size_t* size) {
  if (!SameOwner(owner)) return abi::kInvalidSource;
  spy.peer_capacity = capacity;
  if (capacity > 128 || capacity < spy.payload.size()) return abi::kInvalid;
  std::memcpy(bytes, spy.payload.data(), spy.payload.size());
  *size = spy.peer_oversize_reply ? 129 : spy.payload.size();
  return abi::kOk;
}

std::uint32_t __cdecl Support(const abi::SpikeOwner*, std::uint32_t) { return abi::kUnsupported; }

bridge::FunctionsTable Table() noexcept {
  return {Open, Key, Revoke, Join, Inspect, Mode, Read, Support, bridge::TableEvidence::InjectedSpy};
}

LRESULT CALLBACK ParentProc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) noexcept {
  return DefWindowProcW(hwnd, message, wparam, lparam);
}

bool Pump(live::SourceFixtureHost& host, HWND child,
          source::LoopDecision* key_decision = nullptr, unsigned candidate = 0,
          MSG* key_message = nullptr) noexcept {
  MSG message{};
  bool seen = false;
  while (PeekMessageW(&message, child, 0, 0, PM_REMOVE)) {
    const auto decision = host.BeforeTranslate(message, candidate);
    if (key_decision && (message.message == WM_KEYDOWN || message.message == WM_KEYUP)) {
      *key_decision = decision;
      if (key_message) *key_message = message;
    }
    seen = true;
    if (decision.route == source::LoopRoute::ExistingRoute) TranslateMessage(&message);
    if (decision.route != source::LoopRoute::Handled) DispatchMessageW(&message);
  }
  return seen;
}

bool Named(live::SourceFixtureHost& host, unsigned function, source::KeyPhase phase,
           unsigned candidate = 0) noexcept {
  const HWND child = host.LayerSpyWindow();
  return host.TestNamedProbe(function, phase, candidate) == abi::kOk && Pump(host, child);
}

bool Post(live::SourceFixtureHost& host, WPARAM vkey, unsigned scan,
          source::KeyPhase phase = source::KeyPhase::Down) noexcept {
  std::uint64_t bits = 1 | (std::uint64_t{scan} << 16);
  if (phase != source::KeyPhase::Down) bits |= std::uint64_t{1} << 30;
  if (phase == source::KeyPhase::Up) bits |= std::uint64_t{1} << 31;
  const HWND child = host.LayerSpyWindow();
  return PostMessageW(child, phase == source::KeyPhase::Up ? WM_KEYUP : WM_KEYDOWN,
                      vkey, static_cast<LPARAM>(bits)) && Pump(host, child);
}

template <typename Owner>
bool AwaitClose(Owner& owner, HWND parent) noexcept {
  if (!SetTimer(parent, 71, 5, nullptr)) return false;
  const ULONGLONG deadline = GetTickCount64() + 5000;
  bool closed = false;
  // 正常 GetMessage loop 驱动 poll；没有 UI wait/join/sleep 或 core Busy 自旋。
  while (GetTickCount64() < deadline) {
    const auto result = owner.PollClose();
    if (result.hr == S_OK) { closed = true; break; }
    if (result.hr != E_PENDING) break;
    MSG message{};
    if (GetMessageW(&message, nullptr, 0, 0) <= 0) break;
    DispatchMessageW(&message);
  }
  KillTimer(parent, 71);
  return closed;
}

bool Prepare(live::SourceFixtureHost& host, HWND parent, HINSTANCE instance) noexcept {
  if (!Check(10, host.OpenLayerSpy(parent, instance, Table()) == S_OK)) return false;
  const HWND child = host.LayerSpyWindow();
  Check(11, child && IsChild(parent, child) &&
      GetWindowThreadProcessId(child, nullptr) == GetCurrentThreadId() &&
      reinterpret_cast<const void*>(GetWindowLongPtrW(child, GWLP_USERDATA)) == spy.owner.native_context);
  const HWND foreground = GetForegroundWindow();
  SetFocus(parent);
  Check(12, host.FocusOwnFixture() == S_OK && GetFocus() == child && GetForegroundWindow() == foreground);
  return true;
}

void GateCases(HWND parent, HINSTANCE instance) noexcept {
  guosshell::native_input_spike_sdk::BootstrapHost unverified;
  live::SourceFixtureHost off;
  Check(100, off.OpenSource(parent, instance, unverified) == S_FALSE &&
      off.OpenLayerSpy(parent, instance, Table()) == S_FALSE && spy.opens == 0 &&
      source::WindowSource::LiveTextStores() == 0);
  live::SourceFixtureHost host({true, true});
  Check(101, host.OpenSource(parent, instance, unverified) == E_ACCESSDENIED && spy.opens == 0 &&
      source::WindowSource::LiveTextStores() == 0);
  auto foreign = Table();
  foreign.evidence = bridge::TableEvidence::ParentVerifiedSameSdk;
  Check(102, host.OpenLayerSpy(parent, instance, foreign) == E_ACCESSDENIED);
  Check(103, host.OpenLayerSpy(parent, nullptr, Table()) == E_INVALIDARG && spy.opens == 0);
  host.BeginClose();
  Check(104, host.OpenLayerSpy(parent, instance, Table()) == E_UNEXPECTED && spy.opens == 0);
  Check(105, off.TestUnknownScalarProbe(source::KeyPhase::Down) == abi::kDisabled &&
      host.TestUnknownScalarProbe(source::KeyPhase::Down) == abi::kDisabled && spy.keys == 0);
}

void NumericCases(HWND parent, HINSTANCE instance) noexcept {
  live::SourceFixtureHost host({true, true});
  if (!Prepare(host, parent, instance)) return;
  const HWND child = host.LayerSpyWindow();
  Check(200, host.InspectNumbers().caller == live::CallerEvidence::LayerSpy &&
      host.InspectNumbers().origin == live::OriginEvidence::Unknown);
  // 六组只证明 native 参数／数值输出穿过组件；不冒充真实 Rust fixture binding。
  for (unsigned function = 1; function <= 6; ++function) {
    spy.disposition = function == 2 || function == 3 ? 4U : function >= 4 ? 1U : 2U;
    spy.action = function == 2 ? 1U : function == 3 ? 2U : 0U;
    spy.byte_len = function == 4 ? 4U : function >= 5 ? 1U : 0U;
    Check(210 + function, Named(host, function, source::KeyPhase::Down, 14) &&
        spy.key.raw_source == abi::kRawVerified && spy.key.code_kind == abi::kFunction &&
        spy.key.code_value == function && spy.key.legacy_value == function &&
        spy.key.default_action == 14 && host.InspectNumbers().last.result.action == spy.action);
    const auto physical = spy.key.physical;
    spy.disposition = 2;
    spy.action = spy.byte_len = 0;
    Check(220 + function, Named(host, function, source::KeyPhase::Repeat, 14) &&
        spy.key.physical == physical && spy.key.phase == abi::kRepeat);
    Check(230 + function, Named(host, function, source::KeyPhase::Up, 14) &&
        spy.key.physical == physical && spy.key.phase == abi::kUp);
  }
  Check(240, host.TestNamedProbe(25, source::KeyPhase::Down) == abi::kInvalid &&
      host.TestNamedProbe(1, source::KeyPhase::Down, 17) == abi::kInvalid);
  Check(241, host.PeerMode(1) == abi::kOk && host.ReadPeerBytes().size == 4 &&
      std::memcmp(host.ReadPeerBytes().bytes.data(), spy.payload.data(), 4) == 0);
  std::printf("peer_capacity=%zu\n", spy.peer_capacity);
  Check(256, spy.peer_capacity == 128);
  spy.peer_oversize_reply = true;
  const auto oversized = host.ReadPeerBytes();
  Check(257, oversized.status == abi::kInvalid && oversized.size == 0 &&
      std::all_of(oversized.bytes.begin(), oversized.bytes.end(), [](auto byte) { return byte == 0; }));
  spy.peer_oversize_reply = false;
  const unsigned before = spy.keys;
  MSG other{};
  other.hwnd = parent;
  other.message = WM_KEYDOWN;
  other.wParam = VK_F4;
  Check(242, host.BeforeTranslate(other).route == source::LoopRoute::ExistingRoute && spy.keys == before);
  const UINT blocked[] = {WM_CHAR, WM_DEADCHAR, WM_IME_STARTCOMPOSITION, WM_MOUSEMOVE};
  for (unsigned i = 0; i < 4; ++i) {
    MSG message{};
    message.hwnd = child;
    message.message = blocked[i];
    const auto result = host.BeforeTranslate(message);
    Check(250 + i, result.route == source::LoopRoute::Handled && result.hr == E_NOTIMPL);
  }
  Check(254, host.TextImePointerSupport() == abi::kUnsupported && spy.keys == before &&
      host.InspectNumbers().source_counters.tsf_test_down == 0);
  Check(255, AwaitClose(host, parent) && !IsWindow(child) &&
      host.InspectNumbers().worker_joined == 1 && spy.join_thread.load() != GetCurrentThreadId());
}

void CrossThreadProbeCase(live::SourceFixtureHost& host, HWND parent) noexcept {
  struct Call {
    live::SourceFixtureHost* host = nullptr;
    std::uint32_t unknown = abi::kDisabled;
    std::uint32_t named = abi::kDisabled;
    std::atomic<bool> done{false};
  } call;
  call.host = &host;
  const auto posts = host.InspectNumbers().synthetic_posts;
  const auto keys = spy.keys;
  if (!Check(330, SetTimer(parent, 72, 5, nullptr) != 0)) return;
  const HANDLE thread = CreateThread(nullptr, 0, [](void* context) -> DWORD {
    auto& operation = *static_cast<Call*>(context);
    operation.unknown = operation.host->TestUnknownScalarProbe(source::KeyPhase::Down);
    operation.named = operation.host->TestNamedProbe(1, source::KeyPhase::Down);
    operation.done.store(true, std::memory_order_release);
    return 0;
  }, &call, 0, nullptr);
  if (!Check(331, thread != nullptr)) { KillTimer(parent, 72); return; }
  const ULONGLONG deadline = GetTickCount64() + 5000;
  while (!call.done.load(std::memory_order_acquire) && GetTickCount64() < deadline) {
    MSG message{};
    if (GetMessageW(&message, nullptr, 0, 0) <= 0) break;
    DispatchMessageW(&message);
  }
  KillTimer(parent, 72);
  CloseHandle(thread);
  if (!Check(332, call.done.load(std::memory_order_acquire))) {
    // 未完成时保留调用栈到本测试进程终止，不把 timeout 当线程退出。
    std::fflush(stdout);
    ExitProcess(1);
  }
  Check(333, call.unknown == abi::kInvalidSource && call.named == abi::kInvalidSource &&
      host.InspectNumbers().synthetic_posts == posts && spy.keys == keys);
}

void UnknownScalarCases(live::SourceFixtureHost& host, HWND parent) noexcept {
  const HWND child = host.LayerSpyWindow();
  const auto posts = host.InspectNumbers().synthetic_posts;
  const auto keys = spy.keys;
  const void* const context = spy.owner.native_context;
  Check(320, host.TestUnknownScalarProbe(static_cast<source::KeyPhase>(99)) == abi::kInvalid &&
      host.TestNamedProbe(1, static_cast<source::KeyPhase>(99)) == abi::kInvalid &&
      host.InspectNumbers().synthetic_posts == posts);
  spy.disposition = 4;
  spy.action = 1;
  Check(321, host.TestUnknownScalarProbe(source::KeyPhase::Down) == abi::kOk &&
      host.TestUnknownScalarProbe(source::KeyPhase::Repeat) == abi::kBusy &&
      host.TestNamedProbe(1, source::KeyPhase::Down) == abi::kBusy && spy.keys == keys);
  source::LoopDecision decision{};
  const source::KeyPhase phases[] = {source::KeyPhase::Down, source::KeyPhase::Repeat, source::KeyPhase::Up};
  const UINT mapped = MapVirtualKeyExW(0x41, MAPVK_VK_TO_VSC_EX, GetKeyboardLayout(GetCurrentThreadId()));
  const std::uint64_t physical = bridge::NativeInputBridge::kImmutableNativeDomain |
      (mapped & 0xff) | ((mapped & 0xff00) == 0xe000 ? 0x100ULL : 0ULL);
  MSG delivered{};
  for (unsigned i = 0; i < 3; ++i) {
    const bool queued = i == 0 || host.TestUnknownScalarProbe(phases[i]) == abi::kOk;
    const bool pumped = queued && Pump(host, child, &decision, 14, &delivered);
    const auto numbers = host.InspectNumbers();
    Check(322 + i, pumped && decision.route == source::LoopRoute::Handled &&
        delivered.hwnd == child && delivered.wParam == 0x41 && (mapped & 0xff) != 0 &&
        spy.keys == keys + i + 1 && spy.key.physical == physical &&
        spy.key.phase == i && spy.key.raw_source == abi::kRawUnknown &&
        spy.key.raw_kind == abi::kNoCode && spy.key.code_kind == abi::kNoCode &&
        spy.key.legacy_kind == abi::kNoCode && spy.key.text_ownership == abi::kTextUnverified &&
        spy.key.default_action == 0 && numbers.last.result.action == 0 &&
        numbers.last.result.byte_len == 0 && numbers.last.result.disposition == 0 &&
        numbers.origin == live::OriginEvidence::Unknown);
  }
  const auto numbers = host.InspectNumbers();
  Check(325, physical != 0 && spy.owner.native_context == context &&
      numbers.synthetic_posts == posts + 3 && numbers.source_counters.tsf_test_down == 0 &&
      numbers.source_counters.tsf_down == 0 && numbers.source_counters.tsf_test_up == 0 &&
      numbers.source_counters.tsf_up == 0);
  spy.disposition = 2;
  spy.action = 0;
  CrossThreadProbeCase(host, parent);
}

void RejectCases(HWND parent, HINSTANCE instance) noexcept {
  live::SourceFixtureHost host({true, true});
  if (!Prepare(host, parent, instance)) return;
  const unsigned start = spy.keys;
  spy.status = abi::kBusy;
  Check(300, Named(host, 4, source::KeyPhase::Down));
  spy.status = abi::kOk;
  Check(301, Named(host, 4, source::KeyPhase::Repeat) && spy.keys == start + 1);
  Check(302, Named(host, 4, source::KeyPhase::Up) && spy.keys == start + 2 &&
      spy.key.raw_source == abi::kRawUnknown);
  Check(303, Post(host, 'A', 0x1e) && spy.key.raw_source == abi::kRawUnknown &&
      spy.key.code_kind == 0 && spy.key.legacy_kind == 0);
  Check(304, Post(host, 'A', 0x1e, source::KeyPhase::Up));
  std::array<BYTE, 256> state{};
  state[VK_MENU] = state[VK_RMENU] = state[VK_CONTROL] = state[VK_LCONTROL] = 0x80;
  Check(305, SetKeyboardState(state.data()) && Named(host, 5, source::KeyPhase::Down) &&
      spy.key.raw_source == abi::kRawUnknown);
  state.fill(0);
  Check(306, SetKeyboardState(state.data()) && Named(host, 5, source::KeyPhase::Up));
  UnknownScalarCases(host, parent);
  Check(307, host.InspectNumbers().source_counters.tsf_test_down == 0 && AwaitClose(host, parent));
}

void LedgerCases(HWND parent, HINSTANCE instance) noexcept {
  live::SourceFixtureHost host({true, true});
  if (!Prepare(host, parent, instance)) return;
  spy.status = abi::kBusy;
  bool all = true;
  for (unsigned scan = 1; scan <= 33; ++scan) all = Post(host, VK_F9, scan) && all;
  Check(350, all && !host.InspectNumbers().accepting && spy.revokes != 0 &&
      host.TestNamedProbe(4, source::KeyPhase::Down) == abi::kStopped);
  Check(353, host.TestUnknownScalarProbe(source::KeyPhase::Down) == abi::kStopped);
  Check(351, AwaitClose(host, parent));
  Check(352, host.TestUnknownScalarProbe(source::KeyPhase::Down) == abi::kDisabled);
}

void PendingCases(HWND parent, HINSTANCE instance, bool destructor_transfer) noexcept {
  auto* host = new live::SourceFixtureHost({true, true});
  if (!Prepare(*host, parent, instance)) { delete host; return; }
  IUnknown* held = nullptr;
  Check(400, host->AcquireLayerSpyTextStore(&held) == S_OK && held);
  if (!held) { delete host; return; }
  const HWND child = host->LayerSpyWindow();
  spy.revoke_busy = 2;
  spy.release_join = CreateEventW(nullptr, TRUE, FALSE, nullptr);
  Check(410, host->TestUnknownScalarProbe(source::KeyPhase::Down) == abi::kOk);
  const auto pending = host->BeginClose();
  Check(401, pending.hr == E_PENDING && !pending.actual_shutdown_confirmed &&
      IsWindow(child) && spy.joins.load() == 0 && !host->InspectNumbers().accepting);
  Check(411, host->TestUnknownScalarProbe(source::KeyPhase::Repeat) == abi::kStopped);
  if (destructor_transfer) {
    delete host;
    host = nullptr;
  }
  auto retained = destructor_transfer ? live::SourceFixtureHost::TakeRetainedOwner() : host->DetachForUiPoll();
  delete host;
  Check(402, retained.pending() && retained.PollClose().hr == E_PENDING && IsWindow(child) &&
      retained.InspectNumbers().core_revoked == 1 && spy.joins.load() == 0 &&
      source::WindowSource::LiveTextStores() == 1);
  Check(407, !live::SourceFixtureHost::TakeRetainedOwner().pending());
  {
    live::SourceFixtureHost::RetainedOwner moved(std::move(retained));
    Check(408, moved.pending() && !retained.pending());
  }
  auto recovered = live::SourceFixtureHost::TakeRetainedOwner();
  Check(409, recovered.pending() && recovered.PollClose().hr == E_PENDING && IsWindow(child));
  held->Release();
  const auto started = recovered.PollClose();
  Check(403, started.hr == E_PENDING && started.actual_shutdown_confirmed && !IsWindow(child) &&
      recovered.InspectNumbers().management_started == 1 && recovered.InspectNumbers().status == abi::kBusy);
  Check(404, SetEvent(spy.release_join) != FALSE && AwaitClose(recovered, parent));
  const auto finished = recovered.InspectNumbers();
  Check(405, !recovered.pending() && finished.management_completed == 1 && finished.worker_joined == 1 &&
      spy.joins.load() == 1 && spy.join_thread.load() != GetCurrentThreadId() &&
      source::WindowSource::LiveTextStores() == 0);
  live::SourceFixtureHost later({true, true});
  Check(406, later.OpenLayerSpy(parent, instance, Table()) == E_UNEXPECTED && spy.opens == 1 &&
      !live::SourceFixtureHost::TakeRetainedOwner().pending());
  CloseHandle(spy.release_join);
  spy.release_join = nullptr;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2 || CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED) != S_OK) return 2;
  const HINSTANCE instance = GetModuleHandleW(nullptr);
  WNDCLASSW klass{};
  klass.lpfnWndProc = ParentProc;
  klass.hInstance = instance;
  klass.lpszClassName = L"GuoSSHell.LiveSource.LayerSpy";
  if (!RegisterClassW(&klass)) return 2;
  const HWND parent = CreateWindowExW(0, klass.lpszClassName, L"fixture", WS_OVERLAPPEDWINDOW,
      0, 0, 640, 240, nullptr, nullptr, instance, nullptr);
  if (!parent) return 2;
  std::array<BYTE, 256> saved{};
  const bool saved_ok = GetKeyboardState(saved.data()) != FALSE;
  std::array<BYTE, 256> neutral{};
  if (!saved_ok || !SetKeyboardState(neutral.data())) return 2;
  std::printf("caller=LayerSpy origin=Unknown synthetic_only=1\n");
  if (std::strcmp(argv[1], "--gate") == 0) GateCases(parent, instance);
  else if (std::strcmp(argv[1], "--numbers") == 0) NumericCases(parent, instance);
  else if (std::strcmp(argv[1], "--reject") == 0) RejectCases(parent, instance);
  else if (std::strcmp(argv[1], "--ledger") == 0) LedgerCases(parent, instance);
  else if (std::strcmp(argv[1], "--pending") == 0) PendingCases(parent, instance, false);
  else if (std::strcmp(argv[1], "--receiver") == 0) PendingCases(parent, instance, true);
  else return 2;
  Check(900, SetKeyboardState(saved.data()) != FALSE);
  // 失败时固定 orphan root 留到进程退出，不释放其仍被线程／COM 借用的所有者。
  if (failures) {
    std::printf("checks=%u failures=%u retained_until_process_exit=1\n", checks, failures);
    std::fflush(stdout);
    ExitProcess(1);
  }
  Check(901, DestroyWindow(parent) != FALSE && UnregisterClassW(klass.lpszClassName, instance) != FALSE);
  CoUninitialize();
  std::printf("checks=%u failures=%u\n", checks, failures);
  return failures ? 1 : 0;
}
