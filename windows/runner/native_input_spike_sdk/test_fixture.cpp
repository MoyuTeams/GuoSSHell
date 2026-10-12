// 仅 loader 单元测试的非 Rust DLL；不能作为 SDK/hub 运行证明。
#include "bootstrap_host.h"

namespace abi = guosshell::native_input_spike_abi;
static std::uint64_t core_marker = 0;
static std::uint64_t bootstrap_marker = 0;

#define TEST_EXPORT extern "C" __declspec(dllexport)
TEST_EXPORT void rinf_prepare_isolate_extern() {}
TEST_EXPORT void rinf_start_rust_logic_extern() {}
TEST_EXPORT void rinf_stop_rust_logic_extern() {}
TEST_EXPORT void rinf_send_dart_signal_desktop_ordered() {}
TEST_EXPORT void guossh_native_input_spike_open() {}
TEST_EXPORT void guossh_native_input_spike_key() {}
TEST_EXPORT void guossh_native_input_spike_revoke() {}
TEST_EXPORT void guossh_native_input_spike_join() {}
TEST_EXPORT void guossh_native_input_spike_peer_mode() {}
TEST_EXPORT void guossh_native_input_spike_peer_read() {}
#ifndef TEST_MISSING_EXPORT
TEST_EXPORT void guossh_native_input_spike_support() {}
#endif
TEST_EXPORT std::uint32_t guossh_native_input_spike_inspect(
    const abi::SpikeOwner* owner, abi::SpikeSnapshot* out) {
  if (owner || !out) return abi::kInvalid;
  *out = {};
  out->version = abi::kVersion;
  out->size = static_cast<std::uint32_t>(sizeof(*out));
  out->status = abi::kDisabled;
  out->hub_instance = reinterpret_cast<std::uintptr_t>(&core_marker);
#ifdef TEST_DIRTY_CORE
  out->owners = 1;
#endif
  return abi::kDisabled;
}
TEST_EXPORT std::uint32_t guossh_native_input_sdk_fixture_mode() {
#ifdef TEST_NORMAL_MODE
  return 0;
#else
  return 1;
#endif
}
TEST_EXPORT void guossh_native_input_sdk_probe() {}
TEST_EXPORT std::uint64_t test_only_bootstrap_marker() {
  return reinterpret_cast<std::uintptr_t>(&bootstrap_marker);
}
