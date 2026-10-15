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
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_open(
    const abi::SpikeOpen*, abi::SpikeOwner*) { return abi::kUnsupported; }
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_key(
    const abi::SpikeOwner*, const abi::SpikeKey*, abi::SpikeAdmission*) { return abi::kUnsupported; }
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_revoke(
    const abi::SpikeOwner*) { return abi::kUnsupported; }
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_join(
    const abi::SpikeOwner*) { return abi::kUnsupported; }
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_peer_mode(
    const abi::SpikeOwner*, std::uint32_t) { return abi::kUnsupported; }
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_peer_read(
    const abi::SpikeOwner*, std::uint8_t*, std::size_t, std::size_t*) { return abi::kUnsupported; }
#ifndef TEST_MISSING_EXPORT
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_support(
    const abi::SpikeOwner*, std::uint32_t) { return abi::kUnsupported; }
#endif
#ifndef TEST_MISSING_COMMAND
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_command(
    const abi::SpikeOwner*, const abi::Command*, abi::Reply*) { return abi::kUnsupported; }
#endif
#ifndef TEST_MISSING_COPY
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_copy(
    const abi::SpikeOwner*, const abi::ActionLease*, std::uint8_t*, std::size_t,
    std::size_t*) { return abi::kUnsupported; }
#endif
#ifndef TEST_MISSING_PROJECTION
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_projection(
    const abi::SpikeOwner*, abi::Projection*) { return abi::kUnsupported; }
#endif
TEST_EXPORT std::uint32_t __cdecl guossh_native_input_spike_inspect(
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
#ifdef TEST_DIRTY_TAIL
  out->profile_revision = 1;
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
