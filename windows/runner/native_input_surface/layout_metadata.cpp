#include "layout_metadata.h"

namespace guossh::native_input {
std::shared_ptr<LayoutTable> LayoutTable::Create(std::shared_ptr<NativePermit> permit) noexcept {
  if (!permit) return {};
  void* memory = permit->Memory().Allocate(sizeof(LayoutTable));
  if (!memory) return {};
  auto* table = ::new (memory) LayoutTable(permit);
  try {
    return std::shared_ptr<LayoutTable>(table, &LayoutTable::Destroy, PermitAllocator<LayoutTable>(permit));
  } catch (const std::bad_alloc&) {
    // shared_ptr 构造失败已调用 deleter，不能再析构一次。
    return {};
  }
}
void LayoutTable::Destroy(LayoutTable* table) noexcept {
  auto permit = std::move(table->permit_);
  table->~LayoutTable();
  permit->Memory().Deallocate(table, sizeof(LayoutTable));
}
namespace {
ScalarMetadata Classify(const WCHAR* text, int count) noexcept {
  if (count < 0) return {MetadataKind::Dead, 0};
  if (!count) return {MetadataKind::NoCharacter, 0};
  if (count > static_cast<int>(kResultUnits) || !ValidUtf16(text, count)) return {};
  if (count == 1) return {MetadataKind::Scalar, text[0]};
  if (count == 2 && HighSurrogate(text[0]) && LowSurrogate(text[1]))
    return {MetadataKind::Scalar, 0x10000u + ((text[0] - 0xd800u) << 10) + text[1] - 0xdc00u};
  return {MetadataKind::MultipleScalars, 0};
}
bool Down(const NativeKey& key, UINT vk) noexcept { return (key.queue_state[vk] & 0x80) != 0; }
}

bool NamedKey(UINT vk) noexcept {
  switch (vk) {
    case VK_BACK: case VK_TAB: case VK_RETURN: case VK_ESCAPE: case VK_SPACE:
    case VK_PRIOR: case VK_NEXT: case VK_END: case VK_HOME:
    case VK_LEFT: case VK_UP: case VK_RIGHT: case VK_DOWN:
    case VK_INSERT: case VK_DELETE: case VK_PAUSE: case VK_SNAPSHOT:
    case VK_SHIFT: case VK_LSHIFT: case VK_RSHIFT:
    case VK_CONTROL: case VK_LCONTROL: case VK_RCONTROL:
    case VK_MENU: case VK_LMENU: case VK_RMENU:
    case VK_LWIN: case VK_RWIN: case VK_CAPITAL: case VK_NUMLOCK: case VK_SCROLL:
      return true;
    default: return vk >= VK_F1 && vk <= VK_F24;
  }
}
HRESULT ReadActiveProfile(ITfInputProcessorProfileMgr* manager, uint64_t epoch,
                          ProfileIdentity* out) noexcept {
  if (!manager || !out) return E_POINTER;
  TF_INPUTPROCESSORPROFILE profile{};
  HRESULT hr = manager->GetActiveProfile(GUID_TFCAT_TIP_KEYBOARD, &profile);
  if (FAILED(hr)) return hr;
  *out = {GetKeyboardLayout(GetCurrentThreadId()), profile.clsid, profile.guidProfile,
          profile.langid, profile.dwProfileType, epoch};
  return S_OK;
}
HRESULT FreezeKey(HWND own, const MSG& msg, uint64_t event, const ProfileIdentity& profile,
                  NativeKey* out) noexcept {
  if (!out) return E_POINTER;
  if (msg.hwnd != own || GetWindowThreadProcessId(own, nullptr) != GetCurrentThreadId()) return E_ACCESSDENIED;
  if (msg.message != WM_KEYDOWN && msg.message != WM_KEYUP &&
      msg.message != WM_SYSKEYDOWN && msg.message != WM_SYSKEYUP) return E_INVALIDARG;
  *out = {};
  out->message = msg;
  out->event_id = event;
  out->layout = profile;
  if (GetKeyboardLayout(GetCurrentThreadId()) != profile.hkl) return E_UNEXPECTED;
  if (!GetKeyboardState(out->queue_state.data())) return HRESULT_FROM_WIN32(GetLastError());
  out->scan = static_cast<uint16_t>((msg.lParam >> 16) & 0xff);
  out->extended = (msg.lParam & (1LL << 24)) != 0;
  out->repeat = (msg.lParam & (1LL << 30)) != 0;
  out->up = msg.message == WM_KEYUP || msg.message == WM_SYSKEYUP;
  if (NamedKey(static_cast<UINT>(msg.wParam))) {
    out->raw = out->legacy = {MetadataKind::Named, static_cast<uint32_t>(msg.wParam)};
    out->trusted_raw = true;
    out->metadata_fault = Fault::None;
  }
  return S_OK;
}
Fault LayoutTable::Query(const ProfileIdentity& profile) noexcept {
  verified_ = false;
  profile_ = profile;
  if (!profile.hkl || !permit_) return Fault::LayoutUnverified;
  // 固定数组枚举当前已安装布局，溢出直接拒绝，不读取或修改用户配置。
  std::array<HKL, 64> installed{};
  const int count = GetKeyboardLayoutList(0, nullptr);
  if (count <= 0 || count > static_cast<int>(installed.size()) ||
      GetKeyboardLayoutList(count, installed.data()) != count) return Fault::LayoutStale;
  bool present = false;
  for (int i = 0; i < count; ++i) present |= installed[i] == profile.hkl;
  if (!present) return Fault::LayoutStale;
  for (size_t scan = 0; scan < kScanCount; ++scan) {
    const UINT native_scan = static_cast<UINT>(scan & 255) | (scan >= 256 ? 0xe000 : 0);
    const UINT vk = MapVirtualKeyExW(native_scan, MAPVK_VSC_TO_VK_EX, profile.hkl);
    const UINT reverse = vk ? MapVirtualKeyExW(vk, MAPVK_VK_TO_VSC_EX, profile.hkl) : 0;
    for (size_t level = 0; level < kLayoutStates; ++level) {
      auto& cell = cells_[scan * kLayoutStates + level];
      cell = {};
      cell.vk = vk;
      if (!vk || reverse != native_scan) continue;
      std::array<BYTE, 256> state{};
      if (level & 1) state[VK_SHIFT] = state[VK_LSHIFT] = 0x80;
      if (level & 2) state[VK_CAPITAL] = 1;
      if (level & 4) {
        state[VK_CONTROL] = state[VK_LCONTROL] = 0x80;
        state[VK_MENU] = state[VK_RMENU] = 0x80;
      }
      const int n = ToUnicodeEx(vk, native_scan, state.data(), cell.units.data(),
                                static_cast<int>(kResultUnits), 4, profile.hkl);
      cell.result = Classify(cell.units.data(), n);
      cell.count = n > 0 && n <= static_cast<int>(kResultUnits) ? static_cast<uint8_t>(n) : 0;
      if (n > static_cast<int>(kResultUnits)) return Fault::LayoutUnverified;
    }
  }
  return Fault::None;
}
Fault LayoutTable::Build(const ProfileIdentity& profile, const WorkerEvidence& evidence) noexcept {
  verified_ = false;
  if (!evidence.worker_thread || evidence.worker_thread != GetCurrentThreadId() ||
      evidence.worker_thread == evidence.ui_thread || !evidence.ui_thread ||
      !evidence.born_clean || !evidence.never_translated || !evidence.never_attached ||
      !evidence.no_text_context || !evidence.kernel_isolation_contract ||
      !evidence.ui_dead_toggle_verified) return Fault::LayoutStateNotIsolated;
  const Fault result = Query(profile);
  verified_ = result == Fault::None;
  return result;
}
Fault LayoutTable::DiagnosticQuery(const ProfileIdentity& profile) noexcept {
  return Query(profile);
}
Fault LayoutTable::QueryCanonical(const ProfileIdentity& profile, DWORD ui_thread) noexcept {
  if (!ui_thread || GetCurrentThreadId() == ui_thread) return Fault::LayoutStateNotIsolated;
  return Query(profile);
}
Fault LayoutTable::Lookup(NativeKey* key) const noexcept {
  if (!key) return Fault::LayoutUnverified;
  if (!SameProfile(profile_, key->layout)) return Fault::LayoutStale;
  if (NamedKey(static_cast<UINT>(key->message.wParam))) return Fault::None;
  if (!verified_) return Fault::LayoutStateNotIsolated;
  if (key->message.wParam == VK_PROCESSKEY || key->message.wParam == VK_PACKET) return Fault::LayoutUnverified;
  const size_t scan = key->scan + (key->extended ? 256 : 0);
  if (scan >= kScanCount) return Fault::LayoutUnverified;
  const size_t level = (Down(*key, VK_SHIFT) ? 1 : 0) | ((key->queue_state[VK_CAPITAL] & 1) ? 2 : 0);
  const auto& base = Cell(scan, 0);
  const auto& legacy = Cell(scan, level);
  if (base.vk != key->message.wParam || legacy.vk != base.vk) return Fault::LayoutUnverified;
  // 队列态不能区分物理 LeftCtrl 与 AltGr 合成 Ctrl，保留原位，拒绝猜测。
  if (Down(*key, VK_RMENU) || (Down(*key, VK_CONTROL) && Down(*key, VK_MENU))) return Fault::AltGrUnverified;
  key->raw = base.result;
  key->legacy = legacy.result;
  key->trusted_raw = base.result.kind == MetadataKind::Scalar;
  key->metadata_fault = key->trusted_raw ? Fault::None : Fault::LayoutUnverified;
  return key->metadata_fault;
}
size_t LayoutTable::DeadCells() const noexcept {
  size_t count = 0;
  for (const auto& cell : cells_) count += cell.result.kind == MetadataKind::Dead;
  return count;
}

}  // namespace guossh::native_input
