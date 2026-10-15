#ifndef RUNNER_DESKTOP_NATIVE_INPUT_RUNTIME_MODULE_H_
#define RUNNER_DESKTOP_NATIVE_INPUT_RUNTIME_MODULE_H_

#include <windows.h>

#include "../../../native/hub/desktop_native_input.h"

namespace guosshell::desktop_native_input {

// 函数类型只来自共享 C 头；此表不包含静态链接或第二运行时入口。
struct NormalApi {
  decltype(&guossh_normal_v1_register_trusted_host) register_trusted_host = nullptr;
  decltype(&guossh_normal_v1_bind) bind = nullptr;
  decltype(&guossh_normal_v1_key) key = nullptr;
  decltype(&guossh_normal_v1_composition) composition = nullptr;
  decltype(&guossh_normal_v1_action) action = nullptr;
  decltype(&guossh_normal_v1_claim) claim = nullptr;
  decltype(&guossh_normal_v1_action_payload) action_payload = nullptr;
  decltype(&guossh_normal_v1_ack) ack = nullptr;
  decltype(&guossh_normal_v1_projection) projection = nullptr;
  decltype(&guossh_normal_v1_revoke) revoke = nullptr;
  decltype(&guossh_normal_v1_release_retired) release_retired = nullptr;
  decltype(&guossh_normal_v1_reserve_payload) reserve_payload = nullptr;
  decltype(&guossh_normal_v1_payload) payload = nullptr;
  decltype(&guossh_normal_v1_finish) finish = nullptr;
  decltype(&guossh_normal_v1_text_end_query) text_end_query = nullptr;
  decltype(&guossh_normal_v1_text_end_ack) text_end_ack = nullptr;
  decltype(&guossh_normal_v1_presented_geometry) presented_geometry = nullptr;
  decltype(&guossh_normal_v1_invalidate_geometry) invalidate_geometry = nullptr;
  decltype(&guossh_normal_v1_pointer) pointer = nullptr;
};

enum class AttachStatus {
  kAttached,
  kModuleEnumerationFailed,
  kModuleLimitExceeded,
  kModuleChanged,
  kRuntimeMissing,
  kDuplicateRuntime,
  kDeploymentMismatch,
  kMissingExport,
  kForeignExport,
  kLayoutMismatch,
  kBusy,
  kPinFailed,
};

const char* AttachErrorCode(AttachStatus status);

class RuntimeModule {
 public:
  // 仅枚举本进程已加载模块；成功后进程期 pin。每次调用重新查验唯一性。
  AttachStatus Attach();
  const NormalApi* api() const { return module_ ? &api_ : nullptr; }
  HMODULE module() const { return module_; }

 private:
  HMODULE module_ = nullptr;
  NormalApi api_;
};

}  // namespace guosshell::desktop_native_input

#endif  // RUNNER_DESKTOP_NATIVE_INPUT_RUNTIME_MODULE_H_
