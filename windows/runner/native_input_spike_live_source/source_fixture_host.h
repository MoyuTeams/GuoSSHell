#pragma once

#include "../native_input_spike_bridge/native_input_spike_bridge.h"
#include "../native_input_spike_sdk/bootstrap_host.h"

#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>

namespace guosshell::native_input_spike_live_source {

namespace abi = native_input_spike_abi;
namespace source = native_input_spike;
namespace bridge = native_input_spike_bridge;
namespace sdk = native_input_spike_sdk;

struct FixtureOptions {
  bool explicit_opt_in = false;
  bool dedicated_fixture = false;
};

enum class CallerEvidence : std::uint32_t { None, LayerSpy, ParentVerifiedSdk };
enum class OriginEvidence : std::uint32_t { Unknown };

// 只含固定数值；不暴露 HWND、native_context、capability 或实例地址。
struct Numbers {
  CallerEvidence caller = CallerEvidence::None;
  OriginEvidence origin = OriginEvidence::Unknown;
  std::uint32_t status = abi::kDisabled;
  std::uint32_t observation_count = 0;
  std::uint32_t synthetic_posts = 0;
  std::uint32_t accepting = 0;
  std::uint32_t core_revoked = 0;
  std::uint32_t actual_shutdown_confirmed = 0;
  std::uint32_t management_started = 0;
  std::uint32_t management_completed = 0;
  std::uint32_t join_status = abi::kDisabled;
  std::uint32_t owners = 0;
  std::uint32_t charged_slots = 0;
  std::uint32_t charged_bytes = 0;
  std::uint32_t inflight = 0;
  std::uint32_t stopped = 0;
  std::uint32_t worker_done = 0;
  std::uint32_t worker_joined = 0;
  std::uint64_t remote_applied = 0;
  std::uint64_t next_seq = 0;
  std::uint64_t last_written_seq = 0;
  bridge::NumericObservation last{};
  source::SourceCounters source_counters{};
};

struct PeerBytes {
  static constexpr std::size_t kReadMax = 128;
  std::uint32_t status = abi::kDisabled;
  std::size_t size = 0;
  std::array<std::uint8_t, 512> bytes{};
};

struct CloseResult {
  HRESULT hr = S_FALSE;
  bool core_revoke_confirmed = false;
  bool actual_shutdown_confirmed = false;
  bool management_completed = false;
  std::uint32_t join_status = abi::kDisabled;
};

class SourceFixtureHost final {
  struct Root;

 public:
  // 原生所有权转移，不是可供 Dart 构造的数值句柄。只允许原 STA/UI poll。
  class RetainedOwner final {
   public:
    RetainedOwner() noexcept = default;
    ~RetainedOwner();
    RetainedOwner(RetainedOwner&& other) noexcept;
    RetainedOwner& operator=(RetainedOwner&&) = delete;
    RetainedOwner(const RetainedOwner&) = delete;
    RetainedOwner& operator=(const RetainedOwner&) = delete;
    CloseResult PollClose() noexcept;
    Numbers InspectNumbers() noexcept;
    bool pending() const noexcept { return root_ != nullptr; }

   private:
    friend class SourceFixtureHost;
    explicit RetainedOwner(Root* root) noexcept : root_(root) {}
    Root* root_ = nullptr;
    CloseResult final_{};
    Numbers final_numbers_{};
  };

  explicit SourceFixtureHost(FixtureOptions options = {}) noexcept;
  ~SourceFixtureHost();
  SourceFixtureHost(const SourceFixtureHost&) = delete;
  SourceFixtureHost& operator=(const SourceFixtureHost&) = delete;

  HRESULT OpenSource(HWND parent_owned_hwnd, HINSTANCE instance,
                     const sdk::BootstrapHost& verified_host) noexcept;
  source::LoopDecision BeforeTranslate(const MSG& message,
                                      std::uint32_t candidate_intent = 0) noexcept;
  // 只投递一个公开 named-function/phase/intent 到自己的 child HWND；不证明物理来源。
  std::uint32_t TestNamedProbe(std::uint32_t function, source::KeyPhase phase,
                              std::uint32_t candidate_intent = 0) noexcept;
  // 固定 VK_A／candidate 0 的合成负向 case；不提供字符、布局或物理来源证明。
  std::uint32_t TestUnknownScalarProbe(source::KeyPhase phase) noexcept;
  HRESULT FocusOwnFixture() noexcept;
  Numbers InspectNumbers() noexcept;
  std::uint32_t PeerMode(std::uint32_t mode) noexcept;
  PeerBytes ReadPeerBytes() noexcept;
  std::uint32_t TextImePointerSupport() const noexcept;
  CloseResult BeginClose() noexcept;
  CloseResult PollClose() noexcept;
  RetainedOwner DetachForUiPoll() noexcept;
  // receiver／retained handle 已析构时，原 UI 父可以接回固定的 orphan root。
  static RetainedOwner TakeRetainedOwner() noexcept;

#if defined(GUOSSH_LIVE_SOURCE_LAYER_TEST)
  // 仅编译 probe 时存在；只能标为 LayerSpy，不能替代真实 SDK gate。
  HRESULT OpenLayerSpy(HWND parent_owned_hwnd, HINSTANCE instance,
                       bridge::FunctionsTable functions) noexcept;
  HWND LayerSpyWindow() const noexcept;
  HRESULT AcquireLayerSpyTextStore(IUnknown** out) noexcept;
#endif

 private:
  HRESULT OpenTable(HWND, HINSTANCE, bridge::FunctionsTable) noexcept;
  static bool ResolveVerified(const sdk::BootstrapHost&,
                              bridge::FunctionsTable*) noexcept;
  static Numbers InspectRoot(Root&) noexcept;
  static CloseResult CloseRoot(Root&) noexcept;
  static DWORD WINAPI ManageRetirement(void*) noexcept;
  static void Observe(void*, const bridge::NumericObservation&) noexcept;
  static void Relinquish(Root*) noexcept;
  static Root* retained_root_;
  static std::atomic<DWORD> retained_thread_;
  static std::atomic<bool> lifetime_attempted_;

  const FixtureOptions options_;
  const DWORD ui_thread_;
  Root* root_ = nullptr;
  bool closing_ = false;
  CloseResult final_{};
  Numbers final_numbers_{};
};

}  // namespace guosshell::native_input_spike_live_source
