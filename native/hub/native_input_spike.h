#pragma once

#include <array>
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

constexpr std::uint32_t kVersion = 3;
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
  kProductionNotReady = 11,
  kScopedTransportUnavailable = 12,
  kTextCapacity = 13,
  kActiveKeyBudget = 14,
};

enum Origin : std::uint32_t { kDeclaredSynthetic = 1, kVerifiedNative = 2 };
enum Platform : std::uint32_t {
  kFixturePlatform = 0, kWindows = 1, kMacOS = 2, kLinux = 3,
};

// args、flags 和借用 bytes 的含义由同版本 commands.rs 决定；16 不属于有效命令。
enum Opcode : std::uint32_t {
  kPreedit = 1, kGrantPayload = 2, kPayloadChunk = 3, kFinishPayload = 4,
  kCancelPayload = 5, kIndependentText = 6, kControlEffect = 7,
  kClaimAction = 8, kAckAction = 9, kAckActionAndRevoke = 10,
  kRevokeEffects = 11, kConfirmRetired = 12, kInspectEffects = 13,
  kFixtureChunk = 14, kCancelDeadline = 15, kPointer = 17,
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
  std::uint32_t origin;
  std::uint32_t platform;
  std::uint64_t registration;
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
  std::uint64_t native_event;
  std::uint64_t layout_revision;
  std::uint64_t same_event_state;
};

struct TicketRef {
  std::uint64_t owner;
  std::uint64_t context;
  std::uint64_t generation;
  std::uint64_t transaction;
  std::uint64_t effect;
};

struct PayloadGrant {
  std::uint64_t id;
  std::uint32_t total;
  std::uint32_t chunk;
};

struct ActionLease {
  std::uint64_t owner;
  std::uint64_t context;
  std::uint64_t effect;
  std::uint32_t action;
  std::uint32_t target;
};

struct SpikeAdmission {
  std::uint32_t version;
  std::uint32_t size;
  std::uint32_t status;
  std::uint32_t disposition;
  std::uint64_t seq;
  std::uint32_t action;
  std::uint32_t byte_len;
  std::uint64_t effect;
  TicketRef ticket;
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
  // CPU/transport 已终止；已 claim action 仍需原消费者 ACK，不代表线程尾退出。
  std::uint32_t worker_done;
  // 仅非 UI 管理 caller 实际 join 后为 1。
  std::uint32_t worker_joined;
  std::uint64_t last_written_seq;
  std::uint64_t retained_bytes;
  std::uint64_t applied_effect;
  std::uint64_t frame_revision;
  std::uint32_t source_origin;
  std::uint32_t sync_state;
  std::uint64_t sync_epoch;
  std::uint64_t sync_wakes;
  std::uint64_t remote_received;
  std::uint32_t core_revoked;
  std::uint32_t native_retired;
  std::uint64_t scope_delta;
  std::uint64_t root_scope_delta;
  std::uint64_t profile_revision;
  std::uint32_t cell_width;
  std::uint32_t cell_height;
};

struct Command {
  std::uint32_t version;
  std::uint32_t size;
  std::uint32_t opcode;
  std::uint32_t flags;
  TicketRef ticket;
  PayloadGrant grant;
  std::uint64_t args[6];
  const std::uint8_t* bytes;
  std::size_t length;
};

struct Reply {
  std::uint32_t version;
  std::uint32_t size;
  std::uint32_t status;
  std::uint32_t kind;
  std::uint64_t receipt;
  std::uint64_t effect;
  TicketRef ticket;
  PayloadGrant grant;
  ActionLease action;
};

struct Cell {
  std::uint32_t scalar;
  std::uint32_t foreground;
  std::uint32_t background;
  std::uint32_t attributes;
  std::uint32_t width;
  std::uint32_t selected;
};

struct Projection {
  std::uint32_t version;
  std::uint32_t size;
  std::uint64_t owner;
  std::uint64_t source_generation;
  std::uint64_t frame;
  std::uint64_t effect;
  std::uint32_t rows;
  std::uint32_t cols;
  std::int64_t cursor_row;
  std::uint32_t cursor_column;
  std::uint32_t count;
  Cell cells[36];
};

// 本波只核验 64 位布局；其他目标不能凭相同字段名直接放行。
static_assert(sizeof(void*) == 8, "原型 ABI 尚未核验 32 位目标");
static_assert(std::is_standard_layout<SpikeOwner>::value, "Owner 布局");
static_assert(std::is_standard_layout<SpikeOpen>::value, "Open 布局");
static_assert(std::is_standard_layout<SpikeKey>::value, "Key 布局");
static_assert(std::is_standard_layout<SpikeAdmission>::value, "Admission 布局");
static_assert(std::is_standard_layout<SpikeSnapshot>::value, "Snapshot 布局");
static_assert(sizeof(SpikeOwner) == 48 && alignof(SpikeOwner) == 8, "Owner ABI");
static_assert(sizeof(SpikeOpen) == 56 && alignof(SpikeOpen) == 8, "Open ABI");
static_assert(sizeof(SpikeKey) == 96 && alignof(SpikeKey) == 8, "Key ABI");
static_assert(sizeof(SpikeAdmission) == 80 && alignof(SpikeAdmission) == 8,
              "Admission ABI");
static_assert(sizeof(SpikeSnapshot) == 176 && alignof(SpikeSnapshot) == 8,
              "Snapshot ABI");
static_assert(offsetof(SpikeOwner, identity) == 32, "Owner identity ABI");
static_assert(offsetof(SpikeOpen, fixture_name_len) == 32, "Open length ABI");
static_assert(offsetof(SpikeKey, physical) == 8, "Key physical ABI");
static_assert(offsetof(SpikeKey, default_action) == 68, "Key action ABI");
static_assert(offsetof(SpikeAdmission, seq) == 16, "Admission seq ABI");
static_assert(offsetof(SpikeSnapshot, worker_done) == 64, "Snapshot cleanup ABI");
static_assert(offsetof(SpikeSnapshot, worker_joined) == 68, "Snapshot join ABI");
static_assert(offsetof(SpikeSnapshot, last_written_seq) == 72, "Snapshot seq ABI");

#define GUOSSH_SPIKE_LAYOUT(T, S, A) \
  static_assert(std::is_standard_layout<T>::value && sizeof(T) == S && \
                    alignof(T) == A, #T " ABI")
GUOSSH_SPIKE_LAYOUT(TicketRef, 40, 8);
GUOSSH_SPIKE_LAYOUT(PayloadGrant, 16, 8);
GUOSSH_SPIKE_LAYOUT(ActionLease, 32, 8);
GUOSSH_SPIKE_LAYOUT(Command, 136, 8);
GUOSSH_SPIKE_LAYOUT(Reply, 120, 8);
GUOSSH_SPIKE_LAYOUT(Cell, 24, 4);
GUOSSH_SPIKE_LAYOUT(Projection, 928, 8);
#undef GUOSSH_SPIKE_LAYOUT

// 每个字段锁定实际 repr(C) 协议偏移；不是只核验尾字段或总大小。
#define GUOSSH_SPIKE_OFFSET(T, F, O) static_assert(offsetof(T, F) == O, #T "." #F " ABI")
GUOSSH_SPIKE_OFFSET(SpikeOwner, handle, 0);
GUOSSH_SPIKE_OFFSET(SpikeOwner, native_context, 8);
GUOSSH_SPIKE_OFFSET(SpikeOwner, session, 16);
GUOSSH_SPIKE_OFFSET(SpikeOwner, reserved, 20);
GUOSSH_SPIKE_OFFSET(SpikeOwner, generation, 24);
GUOSSH_SPIKE_OFFSET(SpikeOpen, version, 0);
GUOSSH_SPIKE_OFFSET(SpikeOpen, size, 4);
GUOSSH_SPIKE_OFFSET(SpikeOpen, explicit_opt_in, 8);
GUOSSH_SPIKE_OFFSET(SpikeOpen, reserved, 12);
GUOSSH_SPIKE_OFFSET(SpikeOpen, native_context, 16);
GUOSSH_SPIKE_OFFSET(SpikeOpen, fixture_name, 24);
GUOSSH_SPIKE_OFFSET(SpikeOpen, origin, 40);
GUOSSH_SPIKE_OFFSET(SpikeOpen, platform, 44);
GUOSSH_SPIKE_OFFSET(SpikeOpen, registration, 48);
GUOSSH_SPIKE_OFFSET(SpikeKey, version, 0);
GUOSSH_SPIKE_OFFSET(SpikeKey, size, 4);
GUOSSH_SPIKE_OFFSET(SpikeKey, phase, 16);
GUOSSH_SPIKE_OFFSET(SpikeKey, raw_source, 20);
GUOSSH_SPIKE_OFFSET(SpikeKey, raw_kind, 24);
GUOSSH_SPIKE_OFFSET(SpikeKey, raw_value, 28);
GUOSSH_SPIKE_OFFSET(SpikeKey, dual_source, 32);
GUOSSH_SPIKE_OFFSET(SpikeKey, code_kind, 36);
GUOSSH_SPIKE_OFFSET(SpikeKey, code_value, 40);
GUOSSH_SPIKE_OFFSET(SpikeKey, legacy_kind, 44);
GUOSSH_SPIKE_OFFSET(SpikeKey, legacy_value, 48);
GUOSSH_SPIKE_OFFSET(SpikeKey, modifiers, 52);
GUOSSH_SPIKE_OFFSET(SpikeKey, alt_source, 56);
GUOSSH_SPIKE_OFFSET(SpikeKey, alt_sides, 60);
GUOSSH_SPIKE_OFFSET(SpikeKey, text_ownership, 64);
GUOSSH_SPIKE_OFFSET(SpikeKey, native_event, 72);
GUOSSH_SPIKE_OFFSET(SpikeKey, layout_revision, 80);
GUOSSH_SPIKE_OFFSET(SpikeKey, same_event_state, 88);
GUOSSH_SPIKE_OFFSET(TicketRef, owner, 0);
GUOSSH_SPIKE_OFFSET(TicketRef, context, 8);
GUOSSH_SPIKE_OFFSET(TicketRef, generation, 16);
GUOSSH_SPIKE_OFFSET(TicketRef, transaction, 24);
GUOSSH_SPIKE_OFFSET(TicketRef, effect, 32);
GUOSSH_SPIKE_OFFSET(PayloadGrant, id, 0);
GUOSSH_SPIKE_OFFSET(PayloadGrant, total, 8);
GUOSSH_SPIKE_OFFSET(PayloadGrant, chunk, 12);
GUOSSH_SPIKE_OFFSET(ActionLease, owner, 0);
GUOSSH_SPIKE_OFFSET(ActionLease, context, 8);
GUOSSH_SPIKE_OFFSET(ActionLease, effect, 16);
GUOSSH_SPIKE_OFFSET(ActionLease, action, 24);
GUOSSH_SPIKE_OFFSET(ActionLease, target, 28);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, version, 0);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, size, 4);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, status, 8);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, disposition, 12);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, action, 24);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, byte_len, 28);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, effect, 32);
GUOSSH_SPIKE_OFFSET(SpikeAdmission, ticket, 40);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, version, 0);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, size, 4);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, status, 8);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, owners, 12);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, hub_instance, 16);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, engine_instance, 24);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, remote_applied, 32);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, next_seq, 40);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, charged_slots, 48);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, charged_bytes, 52);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, inflight, 56);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, stopped, 60);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, retained_bytes, 80);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, applied_effect, 88);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, frame_revision, 96);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, source_origin, 104);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, sync_state, 108);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, sync_epoch, 112);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, sync_wakes, 120);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, remote_received, 128);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, core_revoked, 136);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, native_retired, 140);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, scope_delta, 144);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, root_scope_delta, 152);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, profile_revision, 160);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, cell_width, 168);
GUOSSH_SPIKE_OFFSET(SpikeSnapshot, cell_height, 172);
GUOSSH_SPIKE_OFFSET(Command, version, 0);
GUOSSH_SPIKE_OFFSET(Command, size, 4);
GUOSSH_SPIKE_OFFSET(Command, opcode, 8);
GUOSSH_SPIKE_OFFSET(Command, flags, 12);
GUOSSH_SPIKE_OFFSET(Command, ticket, 16);
GUOSSH_SPIKE_OFFSET(Command, grant, 56);
GUOSSH_SPIKE_OFFSET(Command, args, 72);
GUOSSH_SPIKE_OFFSET(Command, bytes, 120);
GUOSSH_SPIKE_OFFSET(Command, length, 128);
GUOSSH_SPIKE_OFFSET(Reply, version, 0);
GUOSSH_SPIKE_OFFSET(Reply, size, 4);
GUOSSH_SPIKE_OFFSET(Reply, status, 8);
GUOSSH_SPIKE_OFFSET(Reply, kind, 12);
GUOSSH_SPIKE_OFFSET(Reply, receipt, 16);
GUOSSH_SPIKE_OFFSET(Reply, effect, 24);
GUOSSH_SPIKE_OFFSET(Reply, ticket, 32);
GUOSSH_SPIKE_OFFSET(Reply, grant, 72);
GUOSSH_SPIKE_OFFSET(Reply, action, 88);
GUOSSH_SPIKE_OFFSET(Cell, scalar, 0);
GUOSSH_SPIKE_OFFSET(Cell, foreground, 4);
GUOSSH_SPIKE_OFFSET(Cell, background, 8);
GUOSSH_SPIKE_OFFSET(Cell, attributes, 12);
GUOSSH_SPIKE_OFFSET(Cell, width, 16);
GUOSSH_SPIKE_OFFSET(Cell, selected, 20);
GUOSSH_SPIKE_OFFSET(Projection, version, 0);
GUOSSH_SPIKE_OFFSET(Projection, size, 4);
GUOSSH_SPIKE_OFFSET(Projection, owner, 8);
GUOSSH_SPIKE_OFFSET(Projection, source_generation, 16);
GUOSSH_SPIKE_OFFSET(Projection, frame, 24);
GUOSSH_SPIKE_OFFSET(Projection, effect, 32);
GUOSSH_SPIKE_OFFSET(Projection, rows, 40);
GUOSSH_SPIKE_OFFSET(Projection, cols, 44);
GUOSSH_SPIKE_OFFSET(Projection, cursor_row, 48);
GUOSSH_SPIKE_OFFSET(Projection, cursor_column, 56);
GUOSSH_SPIKE_OFFSET(Projection, count, 60);
GUOSSH_SPIKE_OFFSET(Projection, cells, 64);
#undef GUOSSH_SPIKE_OFFSET

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
// bytes 只在本次调用借用；合成 fixture 的 ticket 不得签发生产/GUI 来源。
using CommandFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*,
                                                 const Command*, Reply*);
// 原 lease 必须仍 Claimed；锁外复制成功后由原消费者 ACK，不自动写剪贴板。
using CopyFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*,
    const ActionLease*, std::uint8_t*, std::size_t, std::size_t*);
using ProjectionFn = std::uint32_t(GUOSSH_SPIKE_CALL*)(const SpikeOwner*, Projection*);

constexpr char kOpenSymbol[] = "guossh_native_input_spike_open";
constexpr char kKeySymbol[] = "guossh_native_input_spike_key";
constexpr char kRevokeSymbol[] = "guossh_native_input_spike_revoke";
constexpr char kJoinSymbol[] = "guossh_native_input_spike_join";
constexpr char kInspectSymbol[] = "guossh_native_input_spike_inspect";
constexpr char kPeerModeSymbol[] = "guossh_native_input_spike_peer_mode";
constexpr char kPeerReadSymbol[] = "guossh_native_input_spike_peer_read";
constexpr char kSupportSymbol[] = "guossh_native_input_spike_support";
constexpr char kCommandSymbol[] = "guossh_native_input_spike_command";
constexpr char kCopySymbol[] = "guossh_native_input_spike_copy";
constexpr char kProjectionSymbol[] = "guossh_native_input_spike_projection";

// 既有 inspect/peer_mode/peer_read/support 保持索引 4..7；新导出只追加。
constexpr std::array<const char*, 11> kCoreSymbols{
    kOpenSymbol, kKeySymbol, kRevokeSymbol, kJoinSymbol, kInspectSymbol,
    kPeerModeSymbol, kPeerReadSymbol, kSupportSymbol, kCommandSymbol,
    kCopySymbol, kProjectionSymbol};

}  // namespace native_input_spike_abi
}  // namespace guosshell

#undef GUOSSH_SPIKE_CALL
