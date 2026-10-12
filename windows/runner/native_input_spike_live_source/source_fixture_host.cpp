#include "source_fixture_host.h"

#include <objbase.h>
#include <psapi.h>

#include <atomic>
#include <limits>
#include <new>

namespace guosshell::native_input_spike_live_source {
namespace {

bool OwnedParent(HWND parent, HINSTANCE instance) noexcept {
  DWORD process = 0;
  APTTYPE apartment{};
  APTTYPEQUALIFIER qualifier{};
  return parent && instance && IsWindow(parent) &&
      GetWindowThreadProcessId(parent, &process) == GetCurrentThreadId() &&
      process == GetCurrentProcessId() &&
      reinterpret_cast<HINSTANCE>(GetWindowLongPtrW(parent, GWLP_HINSTANCE)) == instance &&
      CoGetApartmentType(&apartment, &qualifier) == S_OK &&
      (apartment == APTTYPE_STA || apartment == APTTYPE_MAINSTA);
}

bool MappedHere(HMODULE module, const MODULEINFO& image, FARPROC function) noexcept {
  if (!function || !image.lpBaseOfDll || !image.SizeOfImage) return false;
  const auto begin = reinterpret_cast<std::uintptr_t>(image.lpBaseOfDll);
  const auto address = reinterpret_cast<std::uintptr_t>(function);
  if (address < begin || address - begin >= image.SizeOfImage) return false;
  MEMORY_BASIC_INFORMATION memory{};
  HMODULE containing = nullptr;
  const auto pointer = reinterpret_cast<const void*>(address);
  return VirtualQuery(pointer, &memory, sizeof(memory)) == sizeof(memory) &&
      memory.State == MEM_COMMIT && memory.Type == MEM_IMAGE &&
      memory.AllocationBase == module &&
      GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                        reinterpret_cast<LPCWSTR>(pointer), &containing) &&
      containing == module;
}

bool ValidSnapshot(std::uint32_t status, const abi::SpikeSnapshot& snapshot) noexcept {
  return status == abi::kOk && snapshot.status == status &&
      snapshot.version == abi::kVersion && snapshot.size == sizeof(snapshot);
}

class Entry final {
 public:
  explicit Entry(std::size_t& depth) noexcept : depth_(depth) { ++depth_; }
  ~Entry() { --depth_; }
 private:
  std::size_t& depth_;
};

void SnapshotNumbers(const abi::SpikeSnapshot& snapshot, Numbers* out) noexcept {
  out->owners = snapshot.owners;
  out->charged_slots = snapshot.charged_slots;
  out->charged_bytes = snapshot.charged_bytes;
  out->inflight = snapshot.inflight;
  out->stopped = snapshot.stopped;
  out->worker_done = snapshot.worker_done;
  out->worker_joined = snapshot.worker_joined;
  out->remote_applied = snapshot.remote_applied;
  out->next_seq = snapshot.next_seq;
  out->last_written_seq = snapshot.last_written_seq;
}

}  // namespace

struct SourceFixtureHost::Root {
  Root(HWND parent_window, bridge::FunctionsTable table) noexcept
      : parent(parent_window), ui_thread(GetCurrentThreadId()), functions(table),
        adapter(window, table, true, {this, Observe}) {}

  std::uint32_t PostSyntheticProbe(UINT vkey, source::KeyPhase phase,
                                   std::uint32_t candidate) noexcept;

  const HWND parent;
  const DWORD ui_thread;
  const bridge::FunctionsTable functions;
  source::WindowSource window;
  bridge::NativeInputBridge adapter;
  std::size_t entry_depth = 0;
  bool closing = false;
  bool actual_shutdown = false;
  bool detached = false;
  bool claimed = false;
  bool observation_full = false;
  Numbers numbers{};
  struct {
    bool occupied = false;
    UINT message = 0;
    WPARAM vkey = 0;
    LPARAM bits = 0;
    std::uint32_t candidate = 0;
  } probe;
  bridge::Retirement retirement{};
  HANDLE management_thread = nullptr;
  // 0 未发布，1 管理线程拥有结果区，2 release 完成且不再访问 root。
  std::atomic<unsigned> handoff{0};
  std::uint32_t joined_status = abi::kDisabled;
  std::uint32_t inspected_status = abi::kDisabled;
  abi::SpikeSnapshot joined_snapshot{};
};

SourceFixtureHost::Root* SourceFixtureHost::retained_root_ = nullptr;
std::atomic<DWORD> SourceFixtureHost::retained_thread_{0};
std::atomic<bool> SourceFixtureHost::lifetime_attempted_{false};

SourceFixtureHost::SourceFixtureHost(FixtureOptions options) noexcept
    : options_(options), ui_thread_(GetCurrentThreadId()) {}

SourceFixtureHost::~SourceFixtureHost() {
  // 不析构 Pending source，不等待管理线程；root 的 sink 从不指向此 receiver。
  if (root_) {
    BeginClose();
    Relinquish(root_);
  }
}

bool SourceFixtureHost::ResolveVerified(const sdk::BootstrapHost& host,
                                        bridge::FunctionsTable* out) noexcept {
  const HMODULE module = host.verified_core_module();
  if (!module || !out) return false;
  MODULEINFO image{};
  if (!K32GetModuleInformation(GetCurrentProcess(), module, &image, sizeof(image))) return false;
  const char* symbols[] = {abi::kOpenSymbol, abi::kKeySymbol, abi::kRevokeSymbol,
      abi::kJoinSymbol, abi::kInspectSymbol, abi::kPeerModeSymbol,
      abi::kPeerReadSymbol, abi::kSupportSymbol};
  std::array<FARPROC, 8> exports{};
  for (std::size_t i = 0; i < exports.size(); ++i) {
    exports[i] = GetProcAddress(module, symbols[i]);
    if (!MappedHere(module, image, exports[i])) return false;
  }
  *out = {reinterpret_cast<abi::OpenFn>(exports[0]),
      reinterpret_cast<abi::KeyFn>(exports[1]),
      reinterpret_cast<abi::RevokeFn>(exports[2]),
      reinterpret_cast<abi::JoinFn>(exports[3]),
      reinterpret_cast<abi::InspectFn>(exports[4]),
      reinterpret_cast<abi::PeerModeFn>(exports[5]),
      reinterpret_cast<abi::PeerReadFn>(exports[6]),
      reinterpret_cast<abi::SupportFn>(exports[7]),
      bridge::TableEvidence::ParentVerifiedSameSdk};
  return true;
}

HRESULT SourceFixtureHost::OpenSource(HWND parent, HINSTANCE instance,
                                      const sdk::BootstrapHost& host) noexcept {
  if (GetCurrentThreadId() != ui_thread_) return RPC_E_WRONG_THREAD;
  if (closing_ || root_) return E_UNEXPECTED;
  if (!options_.explicit_opt_in || !options_.dedicated_fixture) return S_FALSE;
  bridge::FunctionsTable functions;
  if (!ResolveVerified(host, &functions)) return E_ACCESSDENIED;
  return OpenTable(parent, instance, functions);
}

HRESULT SourceFixtureHost::OpenTable(HWND parent, HINSTANCE instance,
                                    bridge::FunctionsTable functions) noexcept {
  if (GetCurrentThreadId() != ui_thread_) return RPC_E_WRONG_THREAD;
  if (closing_ || root_) return E_UNEXPECTED;
  if (!options_.explicit_opt_in || !options_.dedicated_fixture) return S_FALSE;
  if (!OwnedParent(parent, instance) || !functions.available()) return E_INVALIDARG;
  bool expected = false;
  if (!lifetime_attempted_.compare_exchange_strong(expected, true)) return E_UNEXPECTED;
  Root* const root = new (std::nothrow) Root(parent, functions);
  if (!root) return E_OUTOFMEMORY;
  root_ = root;
  retained_root_ = root;
  retained_thread_.store(ui_thread_, std::memory_order_release);
  root->numbers.caller = functions.evidence == bridge::TableEvidence::InjectedSpy
      ? CallerEvidence::LayerSpy : CallerEvidence::ParentVerifiedSdk;
  const Entry entry(root->entry_depth);
  const HRESULT hr = root->window.Create(parent, instance, {true, true, root->adapter.callbacks()});
  if (hr != S_OK) root->closing = true;
  return hr;
}

void SourceFixtureHost::Observe(void* user, const bridge::NumericObservation& row) noexcept {
  auto& root = *static_cast<Root*>(user);
  // 一份最后结果及固定计数，没有 GUI action heap 或无界 history。
  if (root.numbers.observation_count == 128) root.observation_full = true;
  else {
    ++root.numbers.observation_count;
    root.numbers.last = row;
  }
}

source::LoopDecision SourceFixtureHost::BeforeTranslate(const MSG& message,
                                                       std::uint32_t candidate) noexcept {
  if (GetCurrentThreadId() != ui_thread_) {
    return {source::LoopRoute::ExistingRoute, source::StopReason::WrongThread, RPC_E_WRONG_THREAD};
  }
  Root* const root = root_;
  if (!root) return {};
  // 此前不读取 message 的输入字段，也不检查其他窗口的焦点／键态。
  if (message.hwnd != root->window.hwnd()) return {};
  const Entry entry(root->entry_depth);
  if (root->probe.occupied && message.message == root->probe.message &&
      message.wParam == root->probe.vkey && message.lParam == root->probe.bits) {
    candidate = root->probe.candidate;
    root->probe.occupied = false;
  }
  const auto result = root->window.PreTranslate(message, candidate);
  if (root->observation_full) {
    root->closing = true;
    root->window.BeginRevoke();
  }
  return result;
}

std::uint32_t SourceFixtureHost::TestNamedProbe(std::uint32_t function,
    source::KeyPhase phase, std::uint32_t candidate) noexcept {
  if (GetCurrentThreadId() != ui_thread_) return abi::kInvalidSource;
  Root* const root = root_;
  if (!root) return abi::kDisabled;
  if (root->closing || !root->window.accepting()) return abi::kStopped;
  if (function < 1 || function > 24 || candidate > 16) return abi::kInvalid;
  return root->PostSyntheticProbe(VK_F1 + function - 1, phase, candidate);
}

std::uint32_t SourceFixtureHost::TestUnknownScalarProbe(source::KeyPhase phase) noexcept {
  if (GetCurrentThreadId() != ui_thread_) return abi::kInvalidSource;
  Root* const root = root_;
  if (!root) return abi::kDisabled;
  if (root->closing || !root->window.accepting()) return abi::kStopped;
  return root->PostSyntheticProbe(0x41, phase, 0);  // 固定 VK_A，不解释为文字。
}

std::uint32_t SourceFixtureHost::Root::PostSyntheticProbe(UINT vkey,
    source::KeyPhase phase, std::uint32_t candidate) noexcept {
  if (probe.occupied) return abi::kBusy;
  const UINT mapped = MapVirtualKeyExW(vkey, MAPVK_VK_TO_VSC_EX, GetKeyboardLayout(ui_thread));
  const UINT scan = mapped & 0xff;
  if (!scan) return abi::kUnsupported;
  std::uint64_t bits = 1 | (std::uint64_t{scan} << 16);
  if ((mapped & 0xff00) == 0xe000) bits |= std::uint64_t{1} << 24;
  switch (phase) {
    case source::KeyPhase::Down: break;
    case source::KeyPhase::Repeat: bits |= std::uint64_t{1} << 30; break;
    case source::KeyPhase::Up: bits |= (std::uint64_t{1} << 30) | (std::uint64_t{1} << 31); break;
    default: return abi::kInvalid;
  }
  const UINT message = phase == source::KeyPhase::Up ? WM_KEYUP : WM_KEYDOWN;
  const LPARAM key_bits = static_cast<LPARAM>(bits);
  probe = {true, message, vkey, key_bits, candidate};
  if (!PostMessageW(window.hwnd(), message, vkey, key_bits)) {
    probe.occupied = false;
    return abi::kInvalidSource;
  }
  if (numbers.synthetic_posts != (std::numeric_limits<std::uint32_t>::max)()) {
    ++numbers.synthetic_posts;
  }
  return abi::kOk;
}

HRESULT SourceFixtureHost::FocusOwnFixture() noexcept {
  if (GetCurrentThreadId() != ui_thread_) return RPC_E_WRONG_THREAD;
  Root* const root = root_;
  if (!root) return S_FALSE;
  if (root->closing || !root->window.accepting()) return CO_E_OBJNOTCONNECTED;
  const HWND focused = GetFocus();
  if (focused != root->parent && !IsChild(root->parent, focused)) return E_ACCESSDENIED;
  const Entry entry(root->entry_depth);
  SetFocus(root->window.hwnd());
  return GetFocus() == root->window.hwnd() ? S_OK : E_FAIL;
}

Numbers SourceFixtureHost::InspectRoot(Root& root) noexcept {
  if (GetCurrentThreadId() != root.ui_thread) {
    Numbers denied;
    denied.status = abi::kInvalidSource;
    return denied;
  }
  const Entry entry(root.entry_depth);
  Numbers out = root.numbers;
  out.accepting = root.window.accepting() ? 1U : 0U;
  out.core_revoked = root.adapter.revoked() ? 1U : 0U;
  out.actual_shutdown_confirmed = root.actual_shutdown ? 1U : 0U;
  out.source_counters = root.window.counters();
  const unsigned handoff = root.handoff.load(std::memory_order_acquire);
  out.management_started = handoff ? 1U : 0U;
  out.management_completed = handoff == 2 ? 1U : 0U;
  // 发布期间 UI 不与 join／inspect 并发调用 core，也不读管理线程结果区。
  if (handoff == 1) out.status = abi::kBusy;
  else if (handoff == 2) {
    out.join_status = root.joined_status;
    out.status = root.inspected_status;
    if (ValidSnapshot(out.status, root.joined_snapshot)) SnapshotNumbers(root.joined_snapshot, &out);
  } else {
    abi::SpikeOwner owner{};
    abi::SpikeSnapshot snapshot{};
    out.status = root.adapter.CopyCapability(&owner)
        ? root.functions.inspect(&owner, &snapshot) : abi::kDisabled;
    if (ValidSnapshot(out.status, snapshot)) SnapshotNumbers(snapshot, &out);
  }
  return out;
}

Numbers SourceFixtureHost::InspectNumbers() noexcept {
  if (GetCurrentThreadId() != ui_thread_) {
    Numbers denied;
    denied.status = abi::kInvalidSource;
    return denied;
  }
  if (root_) return InspectRoot(*root_);
  return final_numbers_;
}

std::uint32_t SourceFixtureHost::PeerMode(std::uint32_t mode) noexcept {
  if (GetCurrentThreadId() != ui_thread_) return abi::kInvalidSource;
  Root* const root = root_;
  if (!root) return abi::kDisabled;
  if (root->closing || !root->window.accepting()) return abi::kStopped;
  if (mode > 4) return abi::kInvalid;
  const Entry entry(root->entry_depth);
  abi::SpikeOwner owner{};
  return root->adapter.CopyCapability(&owner) ? root->functions.peer_mode(&owner, mode) : abi::kDisabled;
}

PeerBytes SourceFixtureHost::ReadPeerBytes() noexcept {
  PeerBytes out;
  if (GetCurrentThreadId() != ui_thread_) {
    out.status = abi::kInvalidSource;
    return out;
  }
  Root* const root = root_;
  if (!root) return out;
  if (root->closing) { out.status = abi::kStopped; return out; }
  const Entry entry(root->entry_depth);
  abi::SpikeOwner owner{};
  if (!root->adapter.CopyCapability(&owner)) return out;
  out.status = root->functions.peer_read(&owner, out.bytes.data(), PeerBytes::kReadMax, &out.size);
  if (out.status != abi::kOk || out.size > PeerBytes::kReadMax) {
    if (out.size > PeerBytes::kReadMax) out.status = abi::kInvalid;
    out.size = 0;
    out.bytes.fill(0);
  }
  return out;
}

std::uint32_t SourceFixtureHost::TextImePointerSupport() const noexcept {
  return GetCurrentThreadId() == ui_thread_ ? abi::kUnsupported : abi::kInvalidSource;
}

DWORD WINAPI SourceFixtureHost::ManageRetirement(void* context) noexcept {
  auto& root = *static_cast<Root*>(context);
  root.joined_status = root.retirement.JoinOnManagementThread();
  root.inspected_status = root.functions.inspect(&root.retirement.owner, &root.joined_snapshot);
  // Rust join 已返回。发布后绝不再解引用 root，更不调用旧 receiver／Dart。
  root.handoff.store(2, std::memory_order_release);
  return 0;
}

CloseResult SourceFixtureHost::CloseRoot(Root& root) noexcept {
  CloseResult out;
  if (GetCurrentThreadId() != root.ui_thread) { out.hr = RPC_E_WRONG_THREAD; return out; }
  const Entry entry(root.entry_depth);
  root.closing = true;
  out.core_revoke_confirmed = root.adapter.revoked();
  out.actual_shutdown_confirmed = root.actual_shutdown;
  const unsigned handoff = root.handoff.load(std::memory_order_acquire);
  if (handoff) {
    out.hr = E_PENDING;
    if (handoff == 2) {
      out.management_completed = true;
      out.join_status = root.joined_status;
      const auto& snapshot = root.joined_snapshot;
      if (root.joined_status == abi::kOk && ValidSnapshot(root.inspected_status, snapshot) &&
          snapshot.stopped == 1 && snapshot.worker_done == 1 && snapshot.worker_joined == 1 &&
          !snapshot.charged_slots && !snapshot.charged_bytes && !snapshot.inflight) out.hr = S_OK;
      else out.hr = E_FAIL;
    }
    return out;
  }
  root.window.BeginRevoke();
  if (root.entry_depth != 1) { out.hr = E_PENDING; return out; }
  abi::SpikeOwner owner{};
  const bool has_owner = root.adapter.CopyCapability(&owner);
  if (has_owner && !root.adapter.revoked()) {
    const auto callbacks = root.adapter.callbacks();
    const auto ack = callbacks.revoke(callbacks.user, {owner.handle, owner.generation});
    if (ack != source::RevokeAck::Confirmed) { out.hr = E_PENDING; return out; }
  }
  out.core_revoke_confirmed = !has_owner || root.adapter.revoked();
  if (!root.window.core_revoked() && root.window.identity().owner_cookie) {
    const HRESULT hr = root.window.ConfirmRevoke();
    if (hr != S_OK) { out.hr = hr; return out; }
  }
  // Shutdown 完整退栈才设置确认和导出；Pending 永远不析构本地所有者。
  out.hr = root.window.Shutdown();
  if (out.hr != S_OK) return out;
  root.actual_shutdown = true;
  out.actual_shutdown_confirmed = true;
  root.probe.occupied = false;
  if (!has_owner) return out;
  if (!root.adapter.ExportRetirement(&root.retirement)) { out.hr = E_PENDING; return out; }
  root.handoff.store(1, std::memory_order_release);
  root.management_thread = CreateThread(nullptr, 0, ManageRetirement, &root, 0, nullptr);
  if (!root.management_thread) {
    // 没有管理线程发布结果；保留 root，父下次正常 UI poll 可重试创建。
    root.handoff.store(0, std::memory_order_release);
    out.hr = HRESULT_FROM_WIN32(GetLastError());
    return out;
  }
  out.hr = E_PENDING;
  return out;
}

CloseResult SourceFixtureHost::BeginClose() noexcept {
  if (GetCurrentThreadId() != ui_thread_) { CloseResult out; out.hr = RPC_E_WRONG_THREAD; return out; }
  closing_ = true;
  return PollClose();
}

CloseResult SourceFixtureHost::PollClose() noexcept {
  if (GetCurrentThreadId() != ui_thread_) { CloseResult out; out.hr = RPC_E_WRONG_THREAD; return out; }
  if (!root_) return final_;
  closing_ = true;
  Root* const root = root_;
  const auto result = CloseRoot(*root);
  if (result.hr == S_OK) {
    final_ = result;
    final_numbers_ = InspectRoot(*root);
    if (root->management_thread) CloseHandle(root->management_thread);
    retained_root_ = nullptr;
    root_ = nullptr;
    delete root;
  }
  return result;
}

void SourceFixtureHost::Relinquish(Root* root) noexcept {
  if (!root || GetCurrentThreadId() != root->ui_thread) return;
  root->detached = true;
  root->claimed = false;
}

SourceFixtureHost::RetainedOwner SourceFixtureHost::DetachForUiPoll() noexcept {
  if (GetCurrentThreadId() != ui_thread_) return {};
  BeginClose();
  Root* const root = root_;
  Relinquish(root);
  root_ = nullptr;
  if (root) root->claimed = true;
  return RetainedOwner(root);
}

SourceFixtureHost::RetainedOwner SourceFixtureHost::TakeRetainedOwner() noexcept {
  if (retained_thread_.load(std::memory_order_acquire) != GetCurrentThreadId()) return {};
  Root* const root = retained_root_;
  if (!root || !root->detached || root->claimed) return {};
  root->claimed = true;
  return RetainedOwner(root);
}

SourceFixtureHost::RetainedOwner::~RetainedOwner() { Relinquish(root_); }

SourceFixtureHost::RetainedOwner::RetainedOwner(RetainedOwner&& other) noexcept
    : root_(other.root_), final_(other.final_), final_numbers_(other.final_numbers_) {
  other.root_ = nullptr;
}

CloseResult SourceFixtureHost::RetainedOwner::PollClose() noexcept {
  if (!root_) return final_;
  Root* const root = root_;
  const auto result = CloseRoot(*root);
  if (result.hr == S_OK) {
    final_ = result;
    final_numbers_ = InspectRoot(*root);
    if (root->management_thread) CloseHandle(root->management_thread);
    retained_root_ = nullptr;
    root_ = nullptr;
    delete root;
  }
  return result;
}

Numbers SourceFixtureHost::RetainedOwner::InspectNumbers() noexcept {
  return root_ ? InspectRoot(*root_) : final_numbers_;
}

#if defined(GUOSSH_LIVE_SOURCE_LAYER_TEST)
HRESULT SourceFixtureHost::OpenLayerSpy(HWND parent, HINSTANCE instance,
                                       bridge::FunctionsTable functions) noexcept {
  if (functions.evidence != bridge::TableEvidence::InjectedSpy) return E_ACCESSDENIED;
  return OpenTable(parent, instance, functions);
}

HWND SourceFixtureHost::LayerSpyWindow() const noexcept {
  return root_ && GetCurrentThreadId() == ui_thread_ ? root_->window.hwnd() : nullptr;
}

HRESULT SourceFixtureHost::AcquireLayerSpyTextStore(IUnknown** out) noexcept {
  if (GetCurrentThreadId() != ui_thread_) return RPC_E_WRONG_THREAD;
  return root_ ? root_->window.AcquireTextStoreForProbe(out) : CO_E_OBJNOTCONNECTED;
}
#endif

}  // namespace guosshell::native_input_spike_live_source
