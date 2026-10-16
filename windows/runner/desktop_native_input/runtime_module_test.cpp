#include "runtime_module.h"

#include <array>
#include <cstdio>
#include <string>

namespace {
using guosshell::desktop_native_input::AttachStatus;
using guosshell::desktop_native_input::RuntimeModule;
using guosshell::desktop_native_input::NormalApi;

bool Check(bool condition, const char* message) {
  if (!condition) std::fprintf(stderr, "%s\n", message);
  return condition;
}

bool OwnedApi(const NormalApi& api, HMODULE module) {
  const std::array addresses = {
      reinterpret_cast<FARPROC>(api.register_trusted_host),
      reinterpret_cast<FARPROC>(api.bind),
      reinterpret_cast<FARPROC>(api.key),
      reinterpret_cast<FARPROC>(api.composition),
      reinterpret_cast<FARPROC>(api.action),
      reinterpret_cast<FARPROC>(api.claim),
      reinterpret_cast<FARPROC>(api.action_payload),
      reinterpret_cast<FARPROC>(api.ack),
      reinterpret_cast<FARPROC>(api.projection),
      reinterpret_cast<FARPROC>(api.revoke),
      reinterpret_cast<FARPROC>(api.release_retired),
      reinterpret_cast<FARPROC>(api.reserve_payload),
      reinterpret_cast<FARPROC>(api.payload),
      reinterpret_cast<FARPROC>(api.finish),
      reinterpret_cast<FARPROC>(api.text_end_query),
      reinterpret_cast<FARPROC>(api.text_end_ack),
      reinterpret_cast<FARPROC>(api.presented_geometry),
      reinterpret_cast<FARPROC>(api.invalidate_geometry),
      reinterpret_cast<FARPROC>(api.pointer)};
  constexpr const char* symbols[] = {
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
  static_assert(addresses.size() == std::size(symbols));
  for (size_t index = 0; index < addresses.size(); ++index) {
    HMODULE containing = nullptr;
    if (!Check(addresses[index] && addresses[index] == GetProcAddress(module, symbols[index]) &&
                   GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
                                          GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                      reinterpret_cast<LPCWSTR>(addresses[index]), &containing) &&
                   containing == module,
               "全部类型化入口必须指向原 module 的真实导出")) return false;
  }
  return true;
}
}  // namespace

int wmain(int argc, wchar_t** argv) {
  if (argc < 2) return 2;
  const std::wstring scenario = argv[1];
  HMODULE loaded = nullptr;
  HMODULE duplicate = nullptr;
  if (argc >= 3) {
    loaded = LoadLibraryExW(argv[2], nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!Check(loaded != nullptr, "测试 DLL 未加载")) return 1;
  }
  if (argc >= 4 && scenario != L"duplicate_after_attach") {
    duplicate = LoadLibraryExW(argv[3], nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!Check(duplicate != nullptr && duplicate != loaded, "第二模块未实际加载")) return 1;
  }
  RuntimeModule runtime;
  if (scenario == L"foreign" && GetModuleHandleW(L"duplicate.dll")) return 1;
  const auto actual = runtime.Attach();
  if (!Check(scenario != L"foreign" || !GetModuleHandleW(L"duplicate.dll"),
             "转发导出不得隐式加载目标模块")) return 1;
  auto expected = AttachStatus::kAttached;
  if (scenario == L"missing") expected = AttachStatus::kRuntimeMissing;
  else if (scenario == L"wrong_place") expected = AttachStatus::kDeploymentMismatch;
  else if (scenario == L"duplicate") expected = AttachStatus::kDuplicateRuntime;
  else if (scenario == L"missing_normal" || scenario == L"missing_rinf") expected = AttachStatus::kMissingExport;
  else if (scenario == L"bad_layout" || scenario == L"mutated_output") expected = AttachStatus::kLayoutMismatch;
  else if (scenario == L"foreign") expected = AttachStatus::kForeignExport;
  else if (scenario != L"good" && scenario != L"real" &&
           scenario != L"duplicate_after_attach") return 2;
  if (actual != expected) {
    std::fprintf(stderr, "actual=%s expected=%s\n",
                 guosshell::desktop_native_input::AttachErrorCode(actual),
                 guosshell::desktop_native_input::AttachErrorCode(expected));
    return 1;
  }
  if (expected != AttachStatus::kAttached) {
    if (!Check(runtime.api() == nullptr && runtime.module() == nullptr,
               "失败后不得暴露 API")) return 1;
  } else {
    if (!Check(runtime.module() == loaded && runtime.api() != nullptr,
               "必须关联加载的同一个模块")) return 1;
    const auto api = *runtime.api();
    if (!OwnedApi(api, loaded)) return 1;
    // 释放测试调用方的加载引用后，loader 的进程期 pin 仍必须有效。
    FreeLibrary(loaded);
    if (!OwnedApi(api, loaded)) return 1;
    if (!Check(runtime.Attach() == AttachStatus::kAttached &&
                   runtime.module() == loaded,
                "重复 attach 必须幂等关联原 module")) return 1;
    if (!OwnedApi(*runtime.api(), loaded)) return 1;
    if (scenario == L"duplicate_after_attach") {
      if (argc < 4) return 2;
      duplicate = LoadLibraryExW(argv[3], nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
      if (!Check(duplicate && duplicate != loaded, "第二模块未实际加载")) return 1;
      if (!Check(runtime.Attach() == AttachStatus::kDuplicateRuntime &&
                     runtime.api() == nullptr && runtime.module() == nullptr,
                 "已 pin 后重复 attach 仍须重新拒绝第二 runtime")) return 1;
      if (!OwnedApi(api, loaded)) return 1;
    }
    loaded = nullptr;
  }
  if (duplicate) FreeLibrary(duplicate);
  if (loaded) FreeLibrary(loaded);
  std::printf("通过：Windows 模块/API 场景\n");
  return 0;
}
