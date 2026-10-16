#include "bootstrap_host.h"

#include <psapi.h>

#include <algorithm>
#include <atomic>
#include <cstring>

namespace guosshell::native_input_spike_sdk {
namespace {
namespace abi = native_input_spike_abi;

// 永不重置；热重启或重建 adapter 也不能积累探测状态。
std::atomic<bool> process_attempted{false};
constexpr const char* kRinfSymbols[] = {
    "rinf_prepare_isolate_extern", "rinf_start_rust_logic_extern",
    "rinf_stop_rust_logic_extern", "rinf_send_dart_signal_desktop_ordered"};

std::uint32_t Read32(const std::uint8_t* bytes) noexcept {
  return std::uint32_t{bytes[0]} | (std::uint32_t{bytes[1]} << 8) |
         (std::uint32_t{bytes[2]} << 16) | (std::uint32_t{bytes[3]} << 24);
}

bool LocalAbsolutePath(const std::wstring& path) {
  // 不接受 UNC、相对路径、设备路径或 ADS，避免 loader 搜索和网络文件访问。
  return path.size() >= 4 && path.size() < 32768 &&
         ((path[0] >= L'A' && path[0] <= L'Z') ||
          (path[0] >= L'a' && path[0] <= L'z')) &&
         path[1] == L':' && path[2] == L'\\' &&
         path.find(L':', 2) == std::wstring::npos &&
         path.find(L'\0') == std::wstring::npos;
}

bool CanonicalLocalPath(const std::wstring& path, std::wstring* out) {
  if (!LocalAbsolutePath(path)) return false;
  const auto root = path.substr(0, 3);
  if (GetDriveTypeW(root.c_str()) != DRIVE_FIXED) return false;
  std::array<wchar_t, 32768> buffer{};
  const DWORD full_size = GetFullPathNameW(
      path.c_str(), static_cast<DWORD>(buffer.size()), buffer.data(), nullptr);
  if (full_size == 0 || full_size >= buffer.size()) return false;
  const std::wstring full_path(buffer.data(), full_size);
  // 受控构建文件不需要 reparse。按层拒绝，避免 drive 路径绕入 UNC 或设备。
  for (std::size_t index = 3; index <= full_path.size(); ++index) {
    if (index != full_path.size() && full_path[index] != L'\\') continue;
    const auto part = full_path.substr(0, index);
    const DWORD attributes = GetFileAttributesW(part.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES ||
        (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0) {
      return false;
    }
  }
  const HANDLE file = CreateFileW(
      full_path.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_DELETE,
      nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) return false;
  const DWORD size = GetFinalPathNameByHandleW(
      file, buffer.data(), static_cast<DWORD>(buffer.size()),
      FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  CloseHandle(file);
  if (size == 0 || size >= buffer.size()) return false;
  std::wstring canonical(buffer.data(), size);
  if (canonical.compare(0, 4, L"\\\\?\\") == 0) canonical.erase(0, 4);
  if (!LocalAbsolutePath(canonical)) return false;
  *out = std::move(canonical);
  return true;
}
}  // namespace

Status DecodeReply(const std::uint8_t* bytes, std::size_t size,
                   ProbeReply* out) noexcept {
  if (!bytes || !out || size != kResponseBytes ||
      std::memcmp(bytes, "GNSR", 4) != 0 || bytes[4] != 1 || bytes[5] != 0 ||
      bytes[6] != 0 || bytes[7] != 0 || Read32(bytes + 8) != abi::kVersion ||
      Read32(bytes + 12) != 64 || Read32(bytes + 40) != 1 ||
      Read32(bytes + 44) == 0 || Read32(bytes + 44) > 8 ||
      std::all_of(bytes + 16, bytes + 32, [](auto byte) { return byte == 0; })) {
    return Status::kInvalidReply;
  }
  ProbeReply reply;
  std::copy_n(bytes + 16, reply.nonce.size(), reply.nonce.begin());
  reply.marker = Read32(bytes + 32) |
                 (std::uint64_t{Read32(bytes + 36)} << 32);
  reply.attempt = Read32(bytes + 44);
  if (reply.marker == 0 || reply.marker % alignof(std::uint64_t) != 0) {
    return Status::kInvalidReply;
  }
  *out = reply;
  return Status::kOk;
}

Diagnostic BootstrapHost::Fail(Status status) {
  ready_ = false;
  diagnostic_.status = status;
  diagnostic_.same_module = false;
  return diagnostic_;
}

bool BootstrapHost::MappedHere(std::uint64_t address) const {
  if (address < image_begin_ || address >= image_end_) return false;
  const auto pointer = reinterpret_cast<const void*>(
      static_cast<std::uintptr_t>(address));
  MEMORY_BASIC_INFORMATION memory{};
  HMODULE containing = nullptr;
  return VirtualQuery(pointer, &memory, sizeof(memory)) == sizeof(memory) &&
         memory.State == MEM_COMMIT && memory.Type == MEM_IMAGE &&
         memory.AllocationBase == module_ &&
         GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                               GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                           reinterpret_cast<LPCWSTR>(pointer), &containing) &&
         containing == module_;
}

bool BootstrapHost::ResolveAll() {
  diagnostic_.rinf_exports = 0;
  diagnostic_.core_exports = 0;
  const auto resolve = [this](const char* symbol) {
    const FARPROC address = GetProcAddress(module_, symbol);
    return address && MappedHere(reinterpret_cast<std::uintptr_t>(address));
  };
  for (const auto symbol : kRinfSymbols) {
    if (!resolve(symbol)) return false;
    ++diagnostic_.rinf_exports;
  }
  for (const auto symbol : abi::kCoreSymbols) {
    if (!resolve(symbol)) return false;
    ++diagnostic_.core_exports;
  }
  if (!resolve(kModeSymbol) || !resolve(kProbeSymbol)) return false;
  // 使用共享头中的真实签名；其他 core 函数只解析，不调用。
  inspect_ = reinterpret_cast<abi::InspectFn>(
      GetProcAddress(module_, abi::kInspectSymbol));
  mode_ = reinterpret_cast<std::uint32_t(__cdecl*)()>(
      GetProcAddress(module_, kModeSymbol));
  return true;
}

bool BootstrapHost::UniqueRinfModule() const {
  std::array<HMODULE, 256> modules{};
  DWORD needed = 0;
  if (!K32EnumProcessModules(GetCurrentProcess(), modules.data(),
                            static_cast<DWORD>(sizeof(modules)), &needed) ||
      needed == 0 || needed > sizeof(modules) || needed % sizeof(HMODULE) != 0) {
    return false;
  }
  // Windows SDK 的 @Native 使用全局符号；任何另一 module 带同名导出都拒绝。
  for (std::size_t index = 0; index < needed / sizeof(HMODULE); ++index) {
    if (modules[index] == module_) continue;
    for (const auto symbol : kRinfSymbols) {
      if (GetProcAddress(modules[index], symbol)) return false;
    }
  }
  return true;
}

bool BootstrapHost::InspectEmptyCore() {
  abi::SpikeSnapshot snapshot{};
  const auto status = inspect_(nullptr, &snapshot);
  diagnostic_.owners = snapshot.owners;
  diagnostic_.core_abi = snapshot.version;
  diagnostic_.pointer_bits = 64;
  if (status != abi::kDisabled || snapshot.status != abi::kDisabled ||
      snapshot.version != abi::kVersion || snapshot.size != sizeof(snapshot) ||
      snapshot.owners != 0 || snapshot.engine_instance != 0 ||
      snapshot.remote_applied != 0 || snapshot.next_seq != 0 ||
      snapshot.charged_slots != 0 || snapshot.charged_bytes != 0 ||
      snapshot.inflight != 0 || snapshot.stopped != 0 ||
      snapshot.worker_done != 0 || snapshot.worker_joined != 0 ||
      snapshot.last_written_seq != 0 || snapshot.retained_bytes != 0 ||
      snapshot.applied_effect != 0 || snapshot.frame_revision != 0 ||
      snapshot.source_origin != 0 || snapshot.sync_state != 0 ||
      snapshot.sync_epoch != 0 || snapshot.sync_wakes != 0 ||
      snapshot.remote_received != 0 || snapshot.core_revoked != 0 ||
      snapshot.native_retired != 0 || snapshot.scope_delta != 0 ||
      snapshot.root_scope_delta != 0 || snapshot.profile_revision != 0 ||
      snapshot.cell_width != 0 || snapshot.cell_height != 0 ||
      !MappedHere(snapshot.hub_instance) ||
      (core_marker_ != 0 && core_marker_ != snapshot.hub_instance)) {
    return false;
  }
  core_marker_ = snapshot.hub_instance;
  return true;
}

Diagnostic BootstrapHost::Preflight(
    const std::wstring& controlled_path, bool explicit_opt_in,
    const std::array<std::uint8_t, 16>& nonce) {
  if (!explicit_opt_in) return Fail(Status::kDisabled);
  if (process_attempted.exchange(true)) return Fail(Status::kAlreadyAttempted);
  if (std::all_of(nonce.begin(), nonce.end(), [](auto byte) { return byte == 0; }) ||
      !CanonicalLocalPath(controlled_path, &canonical_path_)) {
    return Fail(Status::kInvalidPath);
  }
  nonce_ = nonce;
  module_ = LoadLibraryExW(canonical_path_.c_str(), nullptr,
                          LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR |
                              LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
  if (!module_) return Fail(Status::kLoadFailed);
  // 本组件从不 FreeLibrary：失败后也不把已映射库交给 GUI close 卸载。
  MODULEINFO image{};
  if (!K32GetModuleInformation(GetCurrentProcess(), module_, &image,
                               sizeof(image))) {
    return Fail(Status::kForeignModule);
  }
  image_begin_ = reinterpret_cast<std::uintptr_t>(image.lpBaseOfDll);
  image_end_ = image_begin_ + image.SizeOfImage;
  if (!ResolveAll()) return Fail(Status::kMissingExports);
  HMODULE pinned = nullptr;
  if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                             GET_MODULE_HANDLE_EX_FLAG_PIN,
                         reinterpret_cast<LPCWSTR>(mode_), &pinned) ||
      pinned != module_ || !UniqueRinfModule()) {
    return Fail(Status::kForeignModule);
  }
  if (mode_() != 1) return Fail(Status::kNotFixture);
  diagnostic_.fixture_only = true;
  if (!InspectEmptyCore()) return Fail(Status::kDirtyCore);
  diagnostic_.status = Status::kOk;
  ready_ = true;
  return diagnostic_;
}

Diagnostic BootstrapHost::Verify(const std::uint8_t* bytes, std::size_t size) {
  if (!ready_ || verification_attempted_) return Fail(Status::kAlreadyAttempted);
  verification_attempted_ = true;
  ProbeReply reply;
  const auto status = DecodeReply(bytes, size, &reply);
  if (status != Status::kOk) return Fail(status);
  if (reply.nonce != nonce_) return Fail(Status::kNonceMismatch);
  // 由 SDK 回应的真实 Atomic 地址关联 module，不是 Dart 自报身份或 DLL basename。
  if (!MappedHere(reply.marker)) {
    return Fail(Status::kForeignModule);
  }
  // marker 已通过 FROM_ADDRESS 关联 module，再从该同一 module 重新解析导出。
  if (!ResolveAll()) return Fail(Status::kMissingExports);
  if (mode_() != 1 || !UniqueRinfModule()) return Fail(Status::kForeignModule);
  if (!InspectEmptyCore()) return Fail(Status::kDirtyCore);
  diagnostic_.status = Status::kOk;
  diagnostic_.same_module = true;
  return diagnostic_;
}
}  // namespace guosshell::native_input_spike_sdk
