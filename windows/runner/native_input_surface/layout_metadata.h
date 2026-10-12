#pragma once

#include "contracts.h"

namespace guossh::native_input {

constexpr size_t kScanCount = 512;
constexpr size_t kLayoutStates = 8;
constexpr size_t kResultUnits = 8;
struct LayoutCell {
  UINT vk{};
  ScalarMetadata result{};
  std::array<WCHAR, kResultUnits> units{};
  uint8_t count{};
};
struct WorkerEvidence {
  DWORD worker_thread{}, ui_thread{};
  bool born_clean{}, never_translated{}, never_attached{}, no_text_context{};
  // 由父管理 worker 的生命周期和实际 dead 前/中/后实验提供，不是本模块自我认证。
  bool kernel_isolation_contract{}, ui_dead_toggle_verified{};
};
class LayoutTable {
 public:
  static std::shared_ptr<LayoutTable> Create(std::shared_ptr<NativePermit>) noexcept;
  // 只在已授额的 clean 管理 worker 执行；该类不创建线程。
  Fault Build(const ProfileIdentity&, const WorkerEvidence&) noexcept;
  // 仅供受控 fixture 在尚无输入队列的线程查询；结果永远不能用于准入。
  Fault DiagnosticQuery(const ProfileIdentity&) noexcept;
  Fault Lookup(NativeKey*) const noexcept;
  bool Verified() const noexcept { return verified_; }
  const LayoutCell& Cell(size_t scan, size_t state) const noexcept { return cells_[scan * kLayoutStates + state]; }
  const ProfileIdentity& Profile() const noexcept { return profile_; }
  size_t DeadCells() const noexcept;
  static constexpr size_t StorageBytes() noexcept { return sizeof(LayoutCell) * kScanCount * kLayoutStates; }
 private:
  explicit LayoutTable(std::shared_ptr<NativePermit> permit) noexcept : permit_(std::move(permit)) {}
  ~LayoutTable() = default;
  static void Destroy(LayoutTable*) noexcept;
  Fault Query(const ProfileIdentity&) noexcept;
  std::array<LayoutCell, kScanCount * kLayoutStates> cells_{};
  ProfileIdentity profile_{};
  std::shared_ptr<NativePermit> permit_;
  bool verified_{};
};

HRESULT ReadActiveProfile(ITfInputProcessorProfileMgr*, uint64_t epoch, ProfileIdentity*) noexcept;
bool NamedKey(UINT vk) noexcept;
// 只能紧接 own HWND 的 MSG 出队调用；不能在异步回调重读 state。
HRESULT FreezeKey(HWND own, const MSG&, uint64_t event, const ProfileIdentity&, NativeKey*) noexcept;

}  // namespace guossh::native_input
