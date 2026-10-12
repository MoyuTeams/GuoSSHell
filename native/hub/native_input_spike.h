#pragma once

#include <cstddef>
#include <cstdint>
#include <type_traits>

// 仅固定、显式开启的桌面原型。布局来自同版本 ffi.rs，不是生产 V2 或 Rinf ABI。
// 调用者保证借用指针有效、对齐且范围足够；数值校验不能证明原生事件来源。
#if defined(_WIN32)
#define GUOSSH_SPIKE_CALL __cdecl
#else
#define GUOSSH_SPIKE_CALL
#endif

namespace guosshell {
namespace native_input_spike_abi {

constexpr std::uint32_t kVersion = 2;
constexpr char kFixtureName[] = "native-input-fixture-v1";

enum Status : std::uint32_t {
  kOk = 0,
  kDisabled = 1,
  kBusy = 2,
  kStaleOwner = 3,
  kInvalid = 4,
  kInvalidSource = 5,
  kUnsupported = 6,
  kStopped = 7,
  kExhausted = 8,
  kEngineFailure = 9,
  kTransportFailure = 10,
};

enum Phase : std::uint32_t { kDown = 0, kRepeat = 1, kUp = 2 };
enum RawSource : std::uint32_t {
  kRawUnknown = 0,
  kRawVerified = 1,
  kRawKnownOutsideDomain = 2,
};
enum CodeKind : std::uint32_t {
  kNoCode = 0,
  kCharacter = 1,
  kEnter = 2,
  kEscape = 3,
  kTab = 4,
  kBackspace = 5,
  kDelete = 6,
  kInsert = 7,
  kHome = 8,
  kEnd = 9,
  kPageUp = 10,
  kPageDown = 11,
  kArrowUp = 12,
  kArrowDown = 13,
  kArrowLeft = 14,
  kArrowRight = 15,
  kFunction = 16,
};
enum Modifier : std::uint32_t {
  kShift = 1,
  kControl = 2,
  kAlt = 4,
  kSuper = 8,
};
enum TextOwnership : std::uint32_t {
  kTextUnverified = 0,
  kTerminalKey = 1,
  kNativeText = 2,
};

struct SpikeOwner {
  std::uint64_t handle;
  const void* native_context;
  std::uint32_t session;
  std::uint32_t reserved;
  std::uint64_t generation;
  std::uint8_t identity[16];
};

struct SpikeOpen {
  std::uint32_t version;
  std::uint32_t size;
  std::uint32_t explicit_opt_in;
  std::uint32_t reserved;
  const void* native_context;
  const std::uint8_t* fixture_name;
  std::size_t fixture_name_len;
};

struct SpikeKey {
  std::uint32_t version;
  std::uint32_t size;
  std::uint64_t physical;
  std::uint32_t phase;
  std::uint32_t raw_source;
  std::uint32_t raw_kind;
  std::uint32_t raw_value;
  std::uint32_t dual_source;
  std::uint32_t code_kind;
  std::uint32_t code_value;
  std::uint32_t legacy_kind;
  std::uint32_t legacy_value;
  std::uint32_t modifiers;
  std::uint32_t alt_source;
  std::uint32_t alt_sides;
  std::uint32_t text_ownership;
  std::uint32_t default_action;
};

struct SpikeAdmission {
  std::uint32_t version;
  std::uint32_t size;
  std::uint32_t status;
  std::uint32_t disposition;
  std::uint64_t seq;
  std::uint32_t action;
  std::uint32_t byte_len;
};

struct SpikeSnapshot {
  std::uint32_t version;
  std::uint32_t size;
  std::uint32_t status;
  std::uint32_t owners;
  std::uint64_t hub_instance;
  std::uint64_t engine_instance;
  std::uint64_t remote_applied;
  std::uint64_t next_seq;
  std::uint32_t charged_slots;
  std::uint32_t charged_bytes;
  std::uint32_t inflight;
  std::uint32_t stopped;
  // 仅队列和 ledger 已清理，不是线程退出或动态库卸载许可。
  std::uint32_t worker_done;
  // 仅非 UI 管理 caller 实际 join 后为 1。
  std::uint32_t worker_joined;
  std::uint64_t last_written_seq;
};

// 本波只核验 64 位布局；其他目标不能凭相同字段名直接放行。
static_assert(sizeof(void*) == 8, "原型 ABI 尚未核验 32 位目标");
static_assert(std::is_standard_layout<SpikeOwner>::value, "Owner 布局");
static_assert(std::is_standard_layout<SpikeOpen>::value, "Open 布局");
static_assert(std::is_standard_layout<SpikeKey>::value, "Key 布局");
static_assert(std::is_standard_layout<SpikeAdmission>::value, "Admission 布局");
static_assert(std::is_standard_layout<SpikeSnapshot>::value, "Snapshot 布局");
static_assert(sizeof(SpikeOwner) == 48 && alignof(SpikeOwner) == 8, "Owner ABI");
static_assert(sizeof(SpikeOpen) == 40 && alignof(SpikeOpen) == 8, "Open ABI");
static_assert(sizeof(SpikeKey) == 72 && alignof(SpikeKey) == 8, "Key ABI");
static_assert(sizeof(SpikeAdmission) == 32 && alignof(SpikeAdmission) == 8,
              "Admission ABI");
static_assert(sizeof(SpikeSnapshot) == 80 && alignof(SpikeSnapshot) == 8,
              "Snapshot ABI");
static_assert(offsetof(SpikeOwner, identity) == 32, "Owner identity ABI");
static_assert(offsetof(SpikeOpen, fixture_name_len) == 32, "Open length ABI");
static_assert(offsetof(SpikeKey, physical) == 8, "Key physical ABI");
static_assert(offsetof(SpikeKey, default_action) == 68, "Key action ABI");
static_assert(offsetof(SpikeAdmission, seq) == 16, "Admission seq ABI");
static_assert(offsetof(SpikeSnapshot, worker_done) == 64, "Snapshot cleanup ABI");
static_assert(offsetof(SpikeSnapshot, worker_joined) == 68, "Snapshot join ABI");
static_assert(offsetof(SpikeSnapshot, last_written_seq) == 72, "Snapshot seq ABI");

// 必须从 Rinf 实际使用的同一 module 解析，不另链接静态 hub 或创建第二个 engine。
using OpenFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOpen*, SpikeOwner*);
using KeyFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*, const SpikeKey*,
                                             SpikeAdmission*);
using RevokeFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*);
// 先 revoke；仅专用非 UI 管理线程调用，可等待 join。不得从 callback／actor 调用。
using JoinFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*);
using InspectFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*,
                                                 SpikeSnapshot*);
using PeerModeFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*,
                                                  std::uint32_t);
using PeerReadFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*,
                                                  std::uint8_t*, std::size_t,
                                                  std::size_t*);
using SupportFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*,
                                                 std::uint32_t);

constexpr char kOpenSymbol[] = "guossh_native_input_spike_open";
constexpr char kKeySymbol[] = "guossh_native_input_spike_key";
constexpr char kRevokeSymbol[] = "guossh_native_input_spike_revoke";
constexpr char kJoinSymbol[] = "guossh_native_input_spike_join";
constexpr char kInspectSymbol[] = "guossh_native_input_spike_inspect";
constexpr char kPeerModeSymbol[] = "guossh_native_input_spike_peer_mode";
constexpr char kPeerReadSymbol[] = "guossh_native_input_spike_peer_read";
constexpr char kSupportSymbol[] = "guossh_native_input_spike_support";

}  // namespace native_input_spike_abi
}  // namespace guosshell

#undef GUOSSH_SPIKE_CALL
