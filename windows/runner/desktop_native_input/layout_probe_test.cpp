#include "../native_input_surface/layout_metadata.h"

#include <cstdio>
#include <cstdlib>

using namespace guossh::native_input;
namespace {
struct Permit final : NativePermit {
  NativeMemory& Memory() noexcept override { return memory; }
  NativeMemory memory;
};
struct Worker {
  ProfileIdentity profile;
  std::shared_ptr<LayoutTable> table;
  Fault result{Fault::LayoutUnverified};
};
DWORD WINAPI Query(void* parameter) {
  auto* worker = static_cast<Worker*>(parameter);
  // 新线程不 Translate、不 AttachThreadInput，也不创建 TSF context。
  worker->result = worker->table->DiagnosticQuery(worker->profile);
  return 0;
}
bool QueryWorker(Worker* worker) {
  HANDLE thread = CreateThread(nullptr, 0, Query, worker, 0, nullptr);
  if (!thread) return false;
  const DWORD wait = WaitForSingleObject(thread, 10000);
  CloseHandle(thread);
  if (wait != WAIT_OBJECT_0) {
    // 测试进程独占本轮资源；不 TerminateThread，不让栈数据先于仍在执行的 worker 释放。
    std::fprintf(stderr, "layout probe timeout\n");
    std::fflush(stderr);
    std::_Exit(2);
  }
  return worker->result == Fault::None && !worker->table->Verified();
}
struct Text {
  int count{};
  std::array<WCHAR, kResultUnits> units{};
  bool operator==(const Text& other) const noexcept { return count == other.count && units == other.units; }
};
std::array<BYTE, 256> State(size_t level) {
  std::array<BYTE, 256> state{};
  if (level & 1) state[VK_SHIFT] = state[VK_LSHIFT] = 0x80;
  if (level & 2) state[VK_CAPITAL] = 1;
  if (level & 4) {
    state[VK_CONTROL] = state[VK_LCONTROL] = 0x80;
    state[VK_MENU] = state[VK_RMENU] = 0x80;
  }
  return state;
}
Text Translate(HKL layout, size_t scan, const LayoutCell& cell, size_t level) {
  auto state = State(level);
  Text output{};
  const UINT native_scan = static_cast<UINT>(scan & 255) | (scan >= 256 ? 0xe000u : 0u);
  output.count = ToUnicodeEx(cell.vk, native_scan, state.data(), output.units.data(),
      static_cast<int>(output.units.size()), 0, layout);
  return output;
}
bool SameCell(const LayoutCell& a, const LayoutCell& b) {
  return a.vk == b.vk && a.result.kind == b.result.kind && a.result.scalar == b.result.scalar &&
      a.count == b.count && a.units == b.units;
}
bool SameTable(const LayoutTable& a, const LayoutTable& b) {
  for (size_t scan = 0; scan < kScanCount; ++scan)
    for (size_t state = 0; state < kLayoutStates; ++state)
      if (!SameCell(a.Cell(scan, state), b.Cell(scan, state))) return false;
  return true;
}
}

int RunLayoutProbe() {
  std::array<HKL, 64> installed{};
  const int count = GetKeyboardLayoutList(0, nullptr);
  if (count <= 0 || count > static_cast<int>(installed.size()) ||
      GetKeyboardLayoutList(count, installed.data()) != count) return 1;
  int failed{}, dead_scenarios{};
  for (int index = 0; index < count; ++index) {
    auto permit = std::make_shared<Permit>();
    ProfileIdentity profile{}; profile.hkl = installed[index]; profile.epoch = 1;
    Worker before{profile, LayoutTable::Create(permit)};
    Worker during{profile, LayoutTable::Create(permit)};
    Worker after{profile, LayoutTable::Create(permit)};
    if (!before.table || !during.table || !after.table || !QueryWorker(&before)) { ++failed; continue; }
    size_t letter_scan = kScanCount, dead_scan = kScanCount, dead_level{};
    for (size_t scan = 1; scan < 256; ++scan) {
      const auto& cell = before.table->Cell(scan, 0);
      if (letter_scan == kScanCount && cell.result.kind == MetadataKind::Scalar && cell.result.scalar == 'a') letter_scan = scan;
      for (size_t level = 0; level < kLayoutStates; ++level) {
        if (dead_scan == kScanCount && before.table->Cell(scan, level).result.kind == MetadataKind::Dead) {
          dead_scan = scan; dead_level = level;
        }
      }
    }
    if (letter_scan == kScanCount) { ++failed; continue; }
    const auto& letter = before.table->Cell(letter_scan, 0);
    const Text plain_before = Translate(profile.hkl, letter_scan, letter, 0);
    if (dead_scan == kScanCount) {
      if (!QueryWorker(&during) || !QueryWorker(&after) || !SameTable(*before.table, *during.table) ||
          !SameTable(*before.table, *after.table) ||
          !(plain_before == Translate(profile.hkl, letter_scan, letter, 0))) ++failed;
      std::printf("layout[%d] flags4-query-before/during/after; dead-cells=0; dead-scenario=UNVERIFIED\n", index);
      continue;
    }
    const auto& dead = before.table->Cell(dead_scan, dead_level);
    const Text control_dead = Translate(profile.hkl, dead_scan, dead, dead_level);
    const Text control_composed = Translate(profile.hkl, letter_scan, letter, 0);
    const Text test_dead = Translate(profile.hkl, dead_scan, dead, dead_level);
    const bool queried = QueryWorker(&during);
    const Text test_composed = Translate(profile.hkl, letter_scan, letter, 0);
    const bool queried_after = QueryWorker(&after);
    const Text plain_after = Translate(profile.hkl, letter_scan, letter, 0);
    const bool okay = queried && queried_after && control_dead.count < 0 && test_dead.count < 0 &&
        control_composed == test_composed && plain_before == plain_after &&
        SameTable(*before.table, *during.table) && SameTable(*before.table, *after.table);
    if (!okay) ++failed;
    ++dead_scenarios;
    std::printf("layout[%d] dead-control/worker/dead-after=%s; worker-trust=unverified\n", index, okay ? "PASS" : "FAIL");
  }
  std::printf("installed-layouts=%d dead-scenarios=%d layout-failures=%d; no physical/TIP evidence inferred\n",
      count, dead_scenarios, failed);
  return failed;
}
