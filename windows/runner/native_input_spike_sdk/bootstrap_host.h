#pragma once

#include <windows.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <string>

#include "../../../native/hub/native_input_spike.h"

namespace guosshell::native_input_spike_sdk {

constexpr std::size_t kRequestBytes = 32;
constexpr std::size_t kResponseBytes = 48;
constexpr char kChannel[] = "guosshell/native_input_spike_sdk";
constexpr char kModeSymbol[] = "guossh_native_input_sdk_fixture_mode";
constexpr char kProbeSymbol[] = "guossh_native_input_sdk_probe";

enum class Status : std::uint32_t {
  kOk = 0,
  kDisabled = 1,
  kAlreadyAttempted = 2,
  kInvalidPath = 3,
  kLoadFailed = 4,
  kMissingExports = 5,
  kNotFixture = 6,
  kForeignModule = 7,
  kDirtyCore = 8,
  kInvalidReply = 9,
  kNonceMismatch = 10,
};

struct ProbeReply {
  std::array<std::uint8_t, 16> nonce{};
  std::uint64_t marker = 0;
  std::uint32_t attempt = 0;
};

// 精确固定布局的 borrowed bytes，不接受变长 metadata 或真实输入。
Status DecodeReply(const std::uint8_t* bytes, std::size_t size,
                   ProbeReply* out) noexcept;

struct Diagnostic {
  Status status = Status::kDisabled;
  std::uint32_t core_abi = 0;
  std::uint32_t pointer_bits = 0;
  std::uint32_t owners = 0;
  std::uint32_t rinf_exports = 0;
  std::uint32_t core_exports = 0;
  bool fixture_only = false;
  bool same_module = false;
};

// 仅专用 entry 的单次 MethodChannel adapter 持有，不注册 WindowSource。
// 所有操作在同一宿主 platform thread 串行执行；不允许并发调用。
class BootstrapHost final {
 public:
  BootstrapHost() = default;
  BootstrapHost(const BootstrapHost&) = delete;
  BootstrapHost& operator=(const BootstrapHost&) = delete;
  Diagnostic Preflight(const std::wstring& controlled_path, bool explicit_opt_in,
                       const std::array<std::uint8_t, 16>& nonce);
  Diagnostic Verify(const std::uint8_t* bytes, std::size_t size);
  const std::wstring& compiled_lib_path() const { return canonical_path_; }
  // 仅交给本进程的 fixture 原生 source 适配器；不是 Dart 数值身份。
  // 公共 SDK 的 nonce／映像核验尚未通过时，不允许取得 core 调用入口。
  // 映像已 PIN 到进程退出；借用者不得卸载或另行加载 hub。
  HMODULE verified_core_module() const noexcept {
    return ready_ && verification_attempted_ &&
                   diagnostic_.status == Status::kOk && diagnostic_.same_module
               ? module_
               : nullptr;
  }

 private:
  bool MappedHere(std::uint64_t address) const;
  bool ResolveAll();
  bool UniqueRinfModule() const;
  bool InspectEmptyCore();
  Diagnostic Fail(Status status);

  HMODULE module_ = nullptr;
  std::uint64_t image_begin_ = 0;
  std::uint64_t image_end_ = 0;
  std::uint64_t core_marker_ = 0;
  native_input_spike_abi::InspectFn inspect_ = nullptr;
  std::uint32_t(__cdecl* mode_)() = nullptr;
  std::array<std::uint8_t, 16> nonce_{};
  std::wstring canonical_path_;
  Diagnostic diagnostic_;
  bool ready_ = false;
  bool verification_attempted_ = false;
};

}  // namespace guosshell::native_input_spike_sdk
