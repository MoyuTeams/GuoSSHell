#include "runtime_module.h"

#include <psapi.h>

#include <algorithm>
#include <array>
#include <cstring>
#include <string>
#include <type_traits>
#include <utility>

namespace guosshell::desktop_native_input {
namespace {

constexpr size_t kModuleLimit = 1024;
constexpr size_t kPathLimit = 32768;
// 来自正常 Rinf 的实际 FFI 与正常 Start/ordered 信号，不调用这些入口。
constexpr const char* kRinfSymbols[] = {
    "rinf_prepare_isolate_extern", "rinf_start_rust_logic_extern",
    "rinf_stop_rust_logic_extern", "rinf_send_dart_signal_desktop_start",
    "rinf_send_dart_signal_desktop_ordered"};
constexpr const char* kNormalSymbols[] = {
    "guossh_normal_v1_register_trusted_host", "guossh_normal_v1_bind",
    "guossh_normal_v1_key", "guossh_normal_v1_composition",
    "guossh_normal_v1_action", "guossh_normal_v1_claim",
    "guossh_normal_v1_action_payload", "guossh_normal_v1_ack",
    "guossh_normal_v1_projection", "guossh_normal_v1_revoke",
    "guossh_normal_v1_release_retired", "guossh_normal_v1_reserve_payload",
    "guossh_normal_v1_payload", "guossh_normal_v1_finish",
    "guossh_normal_v1_text_end_query", "guossh_normal_v1_text_end_ack",
    "guossh_normal_v1_presented_geometry", "guossh_normal_v1_invalidate_geometry",
    "guossh_normal_v1_pointer"};

template <typename T>
T Layout() {
  static_assert(std::is_standard_layout_v<T> && std::is_trivially_copyable_v<T>);
  static_assert(offsetof(T, header) == 0);
  T value{};
  value.header = {GUOSSH_NORMAL_ABI_VERSION, static_cast<uint32_t>(sizeof(T))};
  return value;
}

class ModuleRef {
 public:
  ~ModuleRef() { if (value) FreeLibrary(value); }
  HMODULE value = nullptr;
};

enum class ExportKind { kMissing, kDirect, kForwarded, kInvalid };

// GetProcAddress 会为 PE forwarder 隐式加载目标 DLL。先读取已映射的导出表，
// 不触发 DLL 搜索，也不让错误产物在归属查验之前加载第二份 hub。
class MappedExports {
 public:
  bool Open(HMODULE module) {
    module_ = module;
    MODULEINFO information{};
    if (!GetModuleInformation(GetCurrentProcess(), module, &information,
                              sizeof(information))) return false;
    image_size_ = information.SizeOfImage;
    IMAGE_DOS_HEADER dos{};
    IMAGE_NT_HEADERS nt{};
    if (!Read(0, &dos, sizeof(dos)) || dos.e_magic != IMAGE_DOS_SIGNATURE ||
        dos.e_lfanew <= 0 ||
        !Read(static_cast<DWORD>(dos.e_lfanew), &nt, sizeof(nt)) ||
        nt.Signature != IMAGE_NT_SIGNATURE ||
        nt.FileHeader.SizeOfOptionalHeader < sizeof(nt.OptionalHeader) ||
        nt.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR_MAGIC ||
        nt.OptionalHeader.NumberOfRvaAndSizes <= IMAGE_DIRECTORY_ENTRY_EXPORT) {
      return false;
    }
    directory_ = nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXPORT];
    if (directory_.VirtualAddress == 0 && directory_.Size == 0) return true;
    if (directory_.Size < sizeof(exports_) ||
        !Span(directory_.VirtualAddress, directory_.Size) ||
        !Read(directory_.VirtualAddress, &exports_, sizeof(exports_))) return false;
    constexpr DWORD kExportLimit = 65536;
    return exports_.NumberOfNames <= kExportLimit &&
           exports_.NumberOfFunctions <= kExportLimit &&
           Span(exports_.AddressOfNames, exports_.NumberOfNames * sizeof(DWORD)) &&
           Span(exports_.AddressOfNameOrdinals, exports_.NumberOfNames * sizeof(WORD)) &&
           Span(exports_.AddressOfFunctions, exports_.NumberOfFunctions * sizeof(DWORD));
  }

  ExportKind Find(const char* symbol, FARPROC* address) const {
    DWORD low = 0;
    DWORD high = exports_.NumberOfNames;
    while (low < high) {
      const DWORD index = low + (high - low) / 2;
      DWORD name_rva = 0;
      if (!Read(exports_.AddressOfNames + index * sizeof(DWORD),
                &name_rva, sizeof(name_rva)) || name_rva >= image_size_) {
        return ExportKind::kInvalid;
      }
      // 本模块所需符号都短于此界；长 C++ 名称只需前缀即可完成有序比较。
      std::array<char, 128> name{};
      const size_t length = std::min(name.size(), static_cast<size_t>(image_size_ - name_rva));
      if (!Read(name_rva, name.data(), length)) return ExportKind::kInvalid;
      const int compare = std::strncmp(name.data(), symbol, length);
      if (compare < 0) {
        low = index + 1;
      } else if (compare > 0) {
        high = index;
      } else {
        if (!std::memchr(name.data(), '\0', length)) return ExportKind::kInvalid;
        WORD ordinal = 0;
        DWORD function_rva = 0;
        if (!Read(exports_.AddressOfNameOrdinals + index * sizeof(WORD),
                  &ordinal, sizeof(ordinal)) || ordinal >= exports_.NumberOfFunctions ||
            !Read(exports_.AddressOfFunctions + ordinal * sizeof(DWORD),
                  &function_rva, sizeof(function_rva))) return ExportKind::kInvalid;
        if (function_rva == 0) return ExportKind::kMissing;
        if (function_rva >= directory_.VirtualAddress &&
            function_rva - directory_.VirtualAddress < directory_.Size) {
          return ExportKind::kForwarded;
        }
        if (!Span(function_rva, 1)) return ExportKind::kInvalid;
        *address = reinterpret_cast<FARPROC>(
            reinterpret_cast<uintptr_t>(module_) + function_rva);
        return ExportKind::kDirect;
      }
    }
    return ExportKind::kMissing;
  }

 private:
  bool Span(size_t rva, size_t size) const {
    return rva <= image_size_ && size <= image_size_ - rva;
  }
  bool Read(size_t rva, void* target, size_t size) const {
    SIZE_T copied = 0;
    return Span(rva, size) &&
           ReadProcessMemory(GetCurrentProcess(),
                             reinterpret_cast<const void*>(
                                 reinterpret_cast<uintptr_t>(module_) + rva),
                             target, size, &copied) && copied == size;
  }
  HMODULE module_ = nullptr;
  DWORD image_size_ = 0;
  IMAGE_DATA_DIRECTORY directory_{};
  IMAGE_EXPORT_DIRECTORY exports_{};
};

bool EqualPath(const std::wstring& first, const std::wstring& second) {
  return CompareStringOrdinal(first.c_str(), -1, second.c_str(), -1, TRUE) ==
         CSTR_EQUAL;
}

bool ModulePath(HMODULE module, std::wstring* path) {
  std::array<wchar_t, kPathLimit> buffer{};
  const DWORD length = GetModuleFileNameW(
      module, buffer.data(), static_cast<DWORD>(buffer.size()));
  if (length == 0 || length >= buffer.size()) return false;
  path->assign(buffer.data(), length);
  return true;
}

bool FinalPath(const std::wstring& path, std::wstring* final_path) {
  const HANDLE file = CreateFileW(
      path.c_str(), FILE_READ_ATTRIBUTES,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
  if (file == INVALID_HANDLE_VALUE) return false;
  std::array<wchar_t, kPathLimit> buffer{};
  const DWORD length = GetFinalPathNameByHandleW(
      file, buffer.data(), static_cast<DWORD>(buffer.size()),
      FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  CloseHandle(file);
  if (length == 0 || length >= buffer.size()) return false;
  final_path->assign(buffer.data(), length);
  return true;
}

bool NormalDeployment(HMODULE module) {
  std::wstring executable, loaded, expected, actual;
  if (!ModulePath(nullptr, &executable) || !ModulePath(module, &loaded)) {
    return false;
  }
  const auto separator = executable.find_last_of(L"\\/");
  if (separator == std::wstring::npos) return false;
  const auto deployment = executable.substr(0, separator + 1) + L"hub.dll";
  // 同时比较真实文件路径，不能以 basename 或客户端提供的位置充当证据。
  return EqualPath(deployment, loaded) && FinalPath(deployment, &expected) &&
         FinalPath(loaded, &actual) && EqualPath(expected, actual);
}

AttachStatus Candidate(const MappedExports& exports, const std::wstring& path,
                       bool* candidate) {
  const auto separator = path.find_last_of(L"\\/");
  *candidate = EqualPath(path.substr(separator == std::wstring::npos ? 0 : separator + 1),
                         L"hub.dll");
  // 改名、部分导出或不在正常目录的 hub 仍参与重复检测。
  for (const auto& symbols : {std::pair{kRinfSymbols, std::size(kRinfSymbols)},
                              std::pair{kNormalSymbols, std::size(kNormalSymbols)}}) {
    for (size_t index = 0; index < symbols.second; ++index) {
      FARPROC address = nullptr;
      const auto kind = exports.Find(symbols.first[index], &address);
      if (kind == ExportKind::kInvalid) return AttachStatus::kModuleChanged;
      if (kind != ExportKind::kMissing) *candidate = true;
    }
  }
  return AttachStatus::kAttached;
}

bool OwnedExport(HMODULE module, FARPROC address) {
  MEMORY_BASIC_INFORMATION memory{};
  HMODULE containing = nullptr;
  const auto pointer = reinterpret_cast<LPCWSTR>(address);
  if (VirtualQuery(reinterpret_cast<const void*>(address), &memory,
                   sizeof(memory)) != sizeof(memory) ||
      memory.State != MEM_COMMIT || memory.Type != MEM_IMAGE ||
      memory.AllocationBase != module) return false;
  const DWORD protection = memory.Protect & 0xff;
  return (protection == PAGE_EXECUTE || protection == PAGE_EXECUTE_READ ||
          protection == PAGE_EXECUTE_READWRITE ||
          protection == PAGE_EXECUTE_WRITECOPY) &&
         GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                            pointer, &containing) && containing == module;
}

AttachStatus Resolve(HMODULE module, const MappedExports& exports, NormalApi* api) {
  std::array<FARPROC, std::size(kNormalSymbols)> normal{};
  const auto resolve = [&](const char* symbol, FARPROC* address) {
    const auto kind = exports.Find(symbol, address);
    if (kind == ExportKind::kMissing) return AttachStatus::kMissingExport;
    if (kind != ExportKind::kDirect || !OwnedExport(module, *address)) {
      return AttachStatus::kForeignExport;
    }
    return AttachStatus::kAttached;
  };
  for (const auto symbol : kRinfSymbols) {
    FARPROC address = nullptr;
    const auto status = resolve(symbol, &address);
    if (status != AttachStatus::kAttached) return status;
  }
  for (size_t index = 0; index < normal.size(); ++index) {
    const auto status = resolve(kNormalSymbols[index], &normal[index]);
    if (status != AttachStatus::kAttached) return status;
  }
  api->register_trusted_host = reinterpret_cast<decltype(api->register_trusted_host)>(normal[0]);
  api->bind = reinterpret_cast<decltype(api->bind)>(normal[1]);
  api->key = reinterpret_cast<decltype(api->key)>(normal[2]);
  api->composition = reinterpret_cast<decltype(api->composition)>(normal[3]);
  api->action = reinterpret_cast<decltype(api->action)>(normal[4]);
  api->claim = reinterpret_cast<decltype(api->claim)>(normal[5]);
  api->action_payload = reinterpret_cast<decltype(api->action_payload)>(normal[6]);
  api->ack = reinterpret_cast<decltype(api->ack)>(normal[7]);
  api->projection = reinterpret_cast<decltype(api->projection)>(normal[8]);
  api->revoke = reinterpret_cast<decltype(api->revoke)>(normal[9]);
  api->release_retired = reinterpret_cast<decltype(api->release_retired)>(normal[10]);
  api->reserve_payload = reinterpret_cast<decltype(api->reserve_payload)>(normal[11]);
  api->payload = reinterpret_cast<decltype(api->payload)>(normal[12]);
  api->finish = reinterpret_cast<decltype(api->finish)>(normal[13]);
  api->text_end_query = reinterpret_cast<decltype(api->text_end_query)>(normal[14]);
  api->text_end_ack = reinterpret_cast<decltype(api->text_end_ack)>(normal[15]);
  api->presented_geometry = reinterpret_cast<decltype(api->presented_geometry)>(normal[16]);
  api->invalidate_geometry = reinterpret_cast<decltype(api->invalidate_geometry)>(normal[17]);
  api->pointer = reinterpret_cast<decltype(api->pointer)>(normal[18]);
  return AttachStatus::kAttached;
}

template <typename T, typename Call>
bool RejectLayouts(T* request, const Call& call) {
  request->header.version = 0;
  if (!call(GUOSSH_NORMAL_UNSUPPORTED_VERSION)) return false;
  request->header.version = GUOSSH_NORMAL_ABI_VERSION;
  request->header.size = static_cast<uint32_t>(sizeof(T) - 1);
  if (!call(GUOSSH_NORMAL_INVALID_ARGUMENT)) return false;
  request->header.size = static_cast<uint32_t>(sizeof(T) + 1);
  if (!call(GUOSSH_NORMAL_INVALID_ARGUMENT)) return false;
  request->header.size = static_cast<uint32_t>(sizeof(T));
  return true;
}

AttachStatus CheckRejectedLayouts(const NormalApi& api) {
  // 只传无效首参数，不查找真实 handle，不登记、准入或撤权任何 surface。
  auto registration = Layout<GuosshNormalRegistration>();
  auto context = Layout<GuosshNormalContext>();
  auto bound = Layout<GuosshNormalBound>();
  bound.handle = 0x1357;
  auto admission = Layout<GuosshNormalAdmission>();
  admission.end_fence = 0x2468;
  auto claim_info = Layout<GuosshNormalClaimInfo>();
  claim_info.claim = 0x3579;
  auto projection = Layout<GuosshNormalProjection>();
  projection.row_revision = 0x468a;
  auto text_end = Layout<GuosshNormalTextEnd>();
  text_end.end_fence = 0x579b;
  const auto original_bound = bound;
  const auto original_admission = admission;
  const auto original_claim_info = claim_info;
  const auto original_projection = projection;
  const auto original_text_end = text_end;
  uint64_t effect = 0x68ac;
  uint64_t count = 0x79bd;
  std::array<uint8_t, 4> buffer{0x12, 0x34, 0x56, 0x78};
  const auto original_buffer = buffer;
  const auto unchanged = [&] {
    return std::memcmp(&bound, &original_bound, sizeof(bound)) == 0 &&
           std::memcmp(&admission, &original_admission, sizeof(admission)) == 0 &&
           std::memcmp(&claim_info, &original_claim_info, sizeof(claim_info)) == 0 &&
           std::memcmp(&projection, &original_projection, sizeof(projection)) == 0 &&
           std::memcmp(&text_end, &original_text_end, sizeof(text_end)) == 0 &&
           effect == 0x68ac && count == 0x79bd && buffer == original_buffer;
  };
  if (!RejectLayouts(&registration, [&](uint32_t expected) {
        return api.register_trusted_host(&registration, &bound) == expected && unchanged();
      })) return AttachStatus::kLayoutMismatch;
  auto key = Layout<GuosshNormalKey>();
  auto composition = Layout<GuosshNormalComposition>();
  auto action = Layout<GuosshNormalActionRequest>();
  auto claim = Layout<GuosshNormalClaimRequest>();
  auto copy = Layout<GuosshNormalPayloadCopy>();
  copy.buffer = buffer.data();
  copy.capacity = buffer.size();
  auto ack = Layout<GuosshNormalAck>();
  auto reservation = Layout<GuosshNormalTextReservation>();
  auto chunk = Layout<GuosshNormalTextChunk>();
  auto finish = Layout<GuosshNormalTextFinish>();
  auto geometry = Layout<GuosshNormalPresentedGeometry>();
  auto pointer = Layout<GuosshNormalPointer>();
  if (!RejectLayouts(&context, [&](uint32_t expected) {
        return api.bind(&context, &bound) == expected && unchanged() &&
               api.key(&context, &key, &admission) == expected && unchanged() &&
               api.composition(&context, &composition) == expected && unchanged() &&
               api.action(&context, &action, &effect) == expected && unchanged() &&
               api.claim(&context, &claim, &claim_info) == expected && unchanged() &&
               api.action_payload(&context, &copy, &count) == expected && unchanged() &&
               api.ack(&context, &ack) == expected && unchanged() &&
               api.projection(&context, &projection) == expected && unchanged() &&
               api.revoke(&context) == expected && unchanged() &&
               api.release_retired(&context) == expected && unchanged() &&
               api.reserve_payload(&context, &reservation) == expected && unchanged() &&
               api.payload(&context, &chunk) == expected && unchanged() &&
               api.finish(&context, &finish) == expected && unchanged() &&
               api.text_end_query(&context, &text_end) == expected && unchanged() &&
               api.text_end_ack(&context, &text_end) == expected && unchanged() &&
               api.presented_geometry(&context, &geometry) == expected && unchanged() &&
               api.invalidate_geometry(&context, 1) == expected && unchanged() &&
               api.pointer(&context, &pointer, &admission) == expected && unchanged();
      })) return AttachStatus::kLayoutMismatch;
  return AttachStatus::kAttached;
}

AttachStatus CheckBindLayout(const NormalApi& api) {
  // 没有 source/identity 的查询不创建 surface，也不签发原生身份。
  auto context = Layout<GuosshNormalContext>();
  auto bound = Layout<GuosshNormalBound>();
  if (!RejectLayouts(&bound, [&](uint32_t expected) {
        const auto before = bound;
        return api.bind(&context, &bound) == expected &&
               std::memcmp(&bound, &before, sizeof(bound)) == 0;
      })) return AttachStatus::kLayoutMismatch;
  const auto unchanged = bound;
  const auto status = api.bind(&context, &bound);
  if (status == GUOSSH_NORMAL_BUSY) return AttachStatus::kBusy;
  if (status != GUOSSH_NORMAL_STALE_SOURCE ||
      std::memcmp(&bound, &unchanged, sizeof(bound)) != 0) return AttachStatus::kLayoutMismatch;
  return AttachStatus::kAttached;
}

}  // namespace

AttachStatus RuntimeModule::Attach() {
  module_ = nullptr;
  api_ = {};
  std::array<HMODULE, kModuleLimit> modules{};
  DWORD needed = 0;
  if (!EnumProcessModulesEx(GetCurrentProcess(), modules.data(),
                           static_cast<DWORD>(sizeof(modules)), &needed,
                           LIST_MODULES_ALL)) return AttachStatus::kModuleEnumerationFailed;
  if (needed > sizeof(modules)) return AttachStatus::kModuleLimitExceeded;
  if (needed % sizeof(HMODULE) != 0) return AttachStatus::kModuleEnumerationFailed;
  std::array<ModuleRef, kModuleLimit> references{};
  HMODULE candidate = nullptr;
  MappedExports candidate_exports;
  for (size_t index = 0; index < needed / sizeof(HMODULE); ++index) {
    auto& reference = references[index];
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                           reinterpret_cast<LPCWSTR>(modules[index]),
                           &reference.value) || reference.value != modules[index]) {
      return AttachStatus::kModuleChanged;
    }
    std::wstring path;
    if (!ModulePath(reference.value, &path)) return AttachStatus::kModuleChanged;
    MappedExports exports;
    bool is_candidate = false;
    if (!exports.Open(reference.value)) return AttachStatus::kModuleChanged;
    const auto status = Candidate(exports, path, &is_candidate);
    if (status != AttachStatus::kAttached) return status;
    if (is_candidate) {
      if (candidate) return AttachStatus::kDuplicateRuntime;
      candidate = reference.value;
      candidate_exports = exports;
    }
  }
  if (!candidate) return AttachStatus::kRuntimeMissing;
  if (!NormalDeployment(candidate)) return AttachStatus::kDeploymentMismatch;
  NormalApi resolved;
  auto status = Resolve(candidate, candidate_exports, &resolved);
  if (status != AttachStatus::kAttached) return status;
  status = CheckRejectedLayouts(resolved);
  if (status != AttachStatus::kAttached) return status;
  status = CheckBindLayout(resolved);
  if (status != AttachStatus::kAttached) return status;
  std::array<HMODULE, kModuleLimit> final_modules{};
  DWORD final_needed = 0;
  if (!EnumProcessModulesEx(GetCurrentProcess(), final_modules.data(),
                           static_cast<DWORD>(sizeof(final_modules)), &final_needed,
                           LIST_MODULES_ALL)) return AttachStatus::kModuleEnumerationFailed;
  if (final_needed > sizeof(final_modules)) return AttachStatus::kModuleLimitExceeded;
  if (needed != final_needed ||
      std::memcmp(modules.data(), final_modules.data(), needed) != 0) {
    return AttachStatus::kModuleChanged;
  }
  HMODULE pinned = nullptr;
  if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                             GET_MODULE_HANDLE_EX_FLAG_PIN,
                         reinterpret_cast<LPCWSTR>(candidate), &pinned) ||
      pinned != candidate) return AttachStatus::kPinFailed;
  module_ = pinned;
  api_ = resolved;
  return AttachStatus::kAttached;
}

const char* AttachErrorCode(AttachStatus status) {
  switch (status) {
    case AttachStatus::kAttached: return "ATTACHED";
    case AttachStatus::kModuleEnumerationFailed: return "MODULE_ENUMERATION_FAILED";
    case AttachStatus::kModuleLimitExceeded: return "MODULE_LIMIT_EXCEEDED";
    case AttachStatus::kModuleChanged: return "MODULE_CHANGED";
    case AttachStatus::kRuntimeMissing: return "RUNTIME_MISSING";
    case AttachStatus::kDuplicateRuntime: return "DUPLICATE_RUNTIME";
    case AttachStatus::kDeploymentMismatch: return "DEPLOYMENT_MISMATCH";
    case AttachStatus::kMissingExport: return "MISSING_EXPORT";
    case AttachStatus::kForeignExport: return "FOREIGN_EXPORT";
    case AttachStatus::kLayoutMismatch: return "LAYOUT_MISMATCH";
    case AttachStatus::kBusy: return "RUNTIME_BUSY";
    case AttachStatus::kPinFailed: return "MODULE_PIN_FAILED";
  }
  return "ATTACH_FAILED";
}

}  // namespace guosshell::desktop_native_input
