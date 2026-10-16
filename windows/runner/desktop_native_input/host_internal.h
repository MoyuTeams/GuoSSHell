#pragma once
#include "host.h"
#include "geometry.h"
#include "raw_input.h"
#include "../native_input_surface/native_surface.h"
#include <atomic>
#include <map>
#include <mutex>

namespace guosshell::desktop_native_input {
namespace ni = guossh::native_input;
using Microsoft::WRL::ComPtr;
constexpr size_t kRootLimit = 64;
constexpr size_t kPayloadLimit = 1024 * 1024;
constexpr ULONGLONG kTransitionMilliseconds = 4000;
inline bool TextMessage(UINT message) noexcept {
  if (message >= WM_KEYFIRST && message <= WM_KEYLAST) return true;
  switch (message) {
    case WM_IME_STARTCOMPOSITION: case WM_IME_ENDCOMPOSITION: case WM_IME_COMPOSITION:
    case WM_IME_SETCONTEXT: case WM_IME_NOTIFY: case WM_IME_CONTROL: case WM_IME_COMPOSITIONFULL:
    case WM_IME_SELECT: case WM_IME_CHAR: case WM_IME_REQUEST: case WM_IME_KEYDOWN: case WM_IME_KEYUP: return true;
    default: return false;
  }
}

struct MemoryBudget { std::atomic<size_t> permits{}; };
class Permit final : public ni::NativePermit, public ni::NativeMemory {
 public:
  static std::shared_ptr<Permit> Acquire(const std::shared_ptr<MemoryBudget>&);
  ~Permit() override;
  ni::NativeMemory& Memory() noexcept override { return *this; }
  void* Allocate(size_t) noexcept override;
  void Deallocate(void*, size_t) noexcept override;
 private:
  explicit Permit(std::shared_ptr<MemoryBudget> budget) : budget_(std::move(budget)) {}
  std::shared_ptr<MemoryBudget> budget_;
  std::atomic<size_t> bytes_{};
};

// 单个常驻 clean worker；UI 只提交一个固定请求并轮询结果，不 join。
class LayoutWorker final {
 public:
  bool Start();
  bool Request(ni::ProfileIdentity, std::shared_ptr<ni::LayoutTable>);
  std::shared_ptr<const ni::LayoutTable> Result(const ni::ProfileIdentity&);
  void Stop();
  bool Stopped() const;
  ~LayoutWorker();
 private:
  static DWORD WINAPI Run(void*);
  HANDLE thread_{}, wake_{};
  std::atomic<bool> stopping_{};
  std::mutex mutex_;
  ni::ProfileIdentity requested_{};
  std::shared_ptr<ni::LayoutTable> pending_, ready_;
  bool working_{};
  DWORD ui_thread_{};
};

struct NativeHostState;
class PaneRoot final : public ni::Callbacks, public std::enable_shared_from_this<PaneRoot> {
 public:
  PaneRoot(NativeHostState&, uint64_t, std::string token, uint32_t session,
           uint64_t generation, std::string identity, uint64_t attempt, uint64_t lifecycle);
  ~PaneRoot() override;
  bool Create(const Geometry&);
  bool Focus();
  void Seal(const char* reason = nullptr);
  void Poll();
  bool Release();
  Map Present(const Map&);
  Map TextLayout(const Map&);
  Map Claim(uint64_t);
  Map Complete(const Map&);
  Map TextEnd(const Map&);
  bool Invalidate();
  void Raw(const MSG&);
  void LegacyMessage(const MSG&);
  void PointerMessage(const MSG&);
  static LRESULT CALLBACK WindowProc(HWND, UINT, WPARAM, LPARAM);
  GuosshNormalContext Context() const noexcept;
  Map Reply(const char*) const;
  void NotifyState(const char*);
  void Fail(const char*);
  bool Active() const noexcept { return active_ && !revoking_ && !failed_; }
  const std::string token_;
  const uint64_t id_, attempt_, lifecycle_;
  const uint32_t session_;
  const uint64_t generation_;
  const std::string identity_;
  HWND hwnd_{};
  bool active_{}, revoking_{}, core_revoked_{}, quiet_{}, released_{}, failed_{};
  bool want_focus_{}, want_release_{}, polling_{}, opening_{};
  bool raw_restore_required_{};
  bool pointer_capture_{};
  uint64_t request_{}, last_request_{}, native_request_{}, next_context_{}, raw_epoch_{};
  ULONGLONG deadline_{}, release_deadline_{};
  std::string failure_;
  GuosshNormalBound bound_{Abi<GuosshNormalBound>()};
  uint64_t registration_handle_{};
  bool bound_confirmed_{};
  Geometry geometry_{};
  GuosshNormalProjection projection_{Abi<GuosshNormalProjection>()};
  GuosshNormalTextEnd ending_{Abi<GuosshNormalTextEnd>()};
  std::array<GuosshNormalTextEnd, 64> ended_{};
  size_t ended_index_{};
  uint64_t claim_{}, claim_effect_{};
  uint32_t claim_action_{};
  uint32_t claim_rejection_{};
  uint64_t claim_payload_length_{};
  bool claim_delivered_{};
  std::string claim_token_;

  ni::GateReceipt Gate(const ni::OwnerRef&, const ni::NativeKey&, const ni::TicketRef*) noexcept override;
  bool ReservePayload(const ni::TicketRef&, uint32_t, ni::PayloadGrant*) noexcept override;
  bool PreparePayload(const ni::TicketRef&, uint32_t) noexcept override;
  bool Preedit(const ni::TicketRef&, uint64_t, const WCHAR*, ULONG, LONG, LONG) noexcept override;
  bool Payload(const ni::TicketRef&, const ni::PayloadGrant&, uint32_t, const char*, uint32_t, bool) noexcept override;
  bool Finish(const ni::TicketRef&, uint64_t) noexcept override;
  void Cancel(const ni::TicketRef&, ni::Fault) noexcept override;
  void Faulted(const ni::OwnerRef&, ni::Fault) noexcept override;
  bool CoreRevoke(const ni::OwnerRef&, uint64_t*) noexcept override;
  bool ConfirmRetired(const ni::OwnerRef&, uint64_t) noexcept override;
  bool ContextQuiet(const ni::OwnerRef&, const ni::TicketRef*, uint64_t) noexcept override;
  void Pointer(const ni::OwnerRef&, const ni::PointerEvent&) noexcept override {}
 private:
  friend struct HostTestAccess;
  bool OpenContext();
  bool QueryEnd();
  bool AckEnd(uint64_t context, uint64_t fence);
  bool Ticket(const ni::TicketRef&) const noexcept;
  void ApplyGeometry();
  void ReleasePointerCapture() noexcept;
  void EmitComposition(bool ended = false);
  NativeHostState& host_;
  ni::ContextPool pool_;
  ComPtr<ni::NativeSurface> surface_;
  std::shared_ptr<const ni::LayoutTable> layout_;
  bool layout_requested_{};
  int wheel_remainder_{};
  int horizontal_wheel_remainder_{};
  std::wstring composition_;
  uint64_t composition_revision_{}, composition_ticket_{};
  std::array<BYTE, 256> physical_state_{};
};

struct NativeHostState {
  HWND view{}, wake{};
  const DWORD thread{GetCurrentThreadId()};
  NormalApi api;
  Host::Notify notify;
  std::function<void()> poll_callback;
  std::map<std::string, std::shared_ptr<PaneRoot>> roots;
  std::shared_ptr<MemoryBudget> budget{std::make_shared<MemoryBudget>()};
  ComPtr<ITfThreadMgr> manager;
  TfClientId client{};
  LayoutWorker worker;
  RawInputOwner raw;
  uint64_t root_counter{}, raw_counter{};
  bool closing{}, detached{}, pumping{};
  bool Initialize();
  void Poll();
  bool NativeQuiet(const PaneRoot* except = nullptr) const;
  void Emit(const char*, Map) noexcept;
  static LRESULT CALLBACK WakeProc(HWND, UINT, WPARAM, LPARAM);
};
std::string Token();
std::string Utf8(const WCHAR*, size_t);
}  // namespace guosshell::desktop_native_input
