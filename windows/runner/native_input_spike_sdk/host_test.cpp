// 真实 Windows loader + 明确标注的 DLL test double，不调用 Rinf 或输入函数。
#include "bootstrap_host.h"

#include <algorithm>
#include <cassert>
#include <cstring>
#include <cwchar>

namespace sdk = guosshell::native_input_spike_sdk;
namespace abi = guosshell::native_input_spike_abi;

static void Write32(std::uint8_t* bytes, std::uint32_t value) {
  for (unsigned shift = 0; shift < 4; ++shift) {
    bytes[shift] = static_cast<std::uint8_t>(value >> (shift * 8));
  }
}

int wmain(int argc, wchar_t** argv) {
  assert(argc >= 3);
  std::array<std::uint8_t, 16> nonce{};
  nonce.fill(7);
  sdk::BootstrapHost host;
  assert(host.verified_core_module() == nullptr);
  if (std::wcscmp(argv[1], L"off") == 0) {
    assert(host.Preflight(L"missing-relative.dll", false, nonce).status ==
           sdk::Status::kDisabled);
    return 0;
  }
  if (std::wcscmp(argv[1], L"duplicate") == 0) {
    assert(argc == 4);
    assert(LoadLibraryW(argv[3]));
  }
  const auto preflight = host.Preflight(argv[2], true, nonce);
  assert(host.verified_core_module() == nullptr);
  if (std::wcsncmp(argv[1], L"missing", 7) == 0) {
    assert(preflight.status == sdk::Status::kMissingExports);
    return 0;
  }
  if (std::wcscmp(argv[1], L"normal") == 0) {
    assert(preflight.status == sdk::Status::kNotFixture);
    return 0;
  }
  if (std::wcscmp(argv[1], L"dirty") == 0) {
    assert(preflight.status == sdk::Status::kDirtyCore);
    assert(preflight.owners == 1);
    return 0;
  }
  if (std::wcscmp(argv[1], L"dirty_tail") == 0) {
    assert(preflight.status == sdk::Status::kDirtyCore);
    assert(preflight.owners == 0);
    return 0;
  }
  if (std::wcscmp(argv[1], L"duplicate") == 0) {
    assert(preflight.status == sdk::Status::kForeignModule);
    return 0;
  }
  assert(preflight.status == sdk::Status::kOk && !preflight.same_module);
  assert(preflight.fixture_only && preflight.owners == 0 &&
         preflight.rinf_exports == 4 && preflight.core_exports == abi::kCoreSymbols.size());
  auto module = LoadLibraryW(host.compiled_lib_path().c_str());
  assert(module);
  const auto marker_fn = reinterpret_cast<std::uint64_t(__cdecl*)()>(
      GetProcAddress(module, "test_only_bootstrap_marker"));
  assert(marker_fn);
  std::array<std::uint8_t, sdk::kResponseBytes> bytes{};
  std::memcpy(bytes.data(), "GNSR", 4);
  bytes[4] = 1;
  Write32(bytes.data() + 8, abi::kVersion);
  Write32(bytes.data() + 12, 64);
  std::copy(nonce.begin(), nonce.end(), bytes.begin() + 16);
  auto marker = marker_fn();
  Write32(bytes.data() + 32, static_cast<std::uint32_t>(marker));
  Write32(bytes.data() + 36, static_cast<std::uint32_t>(marker >> 32));
  Write32(bytes.data() + 40, 1);
  Write32(bytes.data() + 44, 1);
  sdk::ProbeReply decoded;
  assert(sdk::DecodeReply(nullptr, bytes.size(), &decoded) ==
         sdk::Status::kInvalidReply);
  assert(sdk::DecodeReply(bytes.data(), 47, &decoded) ==
         sdk::Status::kInvalidReply);
  Write32(bytes.data() + 8, 2);
  assert(sdk::DecodeReply(bytes.data(), bytes.size(), &decoded) == sdk::Status::kInvalidReply);
  Write32(bytes.data() + 8, abi::kVersion);
  if (std::wcscmp(argv[1], L"nonce") == 0) {
    bytes[16] = 8;
    assert(host.Verify(bytes.data(), bytes.size()).status ==
           sdk::Status::kNonceMismatch);
    assert(host.verified_core_module() == nullptr);
  } else if (std::wcscmp(argv[1], L"foreign") == 0) {
    marker = reinterpret_cast<std::uintptr_t>(&host);
    Write32(bytes.data() + 32, static_cast<std::uint32_t>(marker));
    Write32(bytes.data() + 36, static_cast<std::uint32_t>(marker >> 32));
    assert(host.Verify(bytes.data(), bytes.size()).status ==
           sdk::Status::kForeignModule);
    assert(host.verified_core_module() == nullptr);
  } else {
    assert(host.Verify(bytes.data(), bytes.size()).same_module);
    assert(host.verified_core_module() == module);
  }
  assert(host.Verify(bytes.data(), bytes.size()).status ==
         sdk::Status::kAlreadyAttempted);
  assert(host.verified_core_module() == nullptr);
  return 0;
}
