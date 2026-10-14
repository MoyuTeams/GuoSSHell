// 独立进程中的真实 Rust ABI 测试；合成 fixture 不代表 Prepared、GUI actor 或物理输入。
#include "bootstrap_host.h"

#include <cassert>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <thread>

namespace abi = guosshell::native_input_spike_abi;
namespace sdk = guosshell::native_input_spike_sdk;

namespace {
constexpr std::uint64_t kCanary = 0x9a26b35167ced048ULL;
template <typename T> struct Output {
  std::uint64_t before = kCanary;
  T value{};
  std::uint64_t after = kCanary;
  void Check() const { assert(before == kCanary && after == kCanary); }
};

template <typename F> void Wait(F ready) {
  const auto deadline = GetTickCount64() + 5000;
  while (!ready()) {
    assert(GetTickCount64() < deadline);
    Sleep(1);  // 仅无窗口的独立测试 caller；不是正常 UI 的重试策略。
  }
}

template <typename F> F Resolve(HMODULE module, const char* symbol) {
  const auto address = GetProcAddress(module, symbol);
  HMODULE containing = nullptr;
  assert(address && GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
      GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
      reinterpret_cast<LPCWSTR>(address), &containing) && containing == module);
  return reinterpret_cast<F>(address);
}

abi::Command Request(std::uint32_t opcode) {
  abi::Command command{};
  command.version = abi::kVersion;
  command.size = sizeof(command);
  command.opcode = opcode;
  return command;
}

abi::Reply Call(abi::CommandFn function, const abi::SpikeOwner& owner,
                const abi::Command& command, std::uint32_t expected = abi::kOk) {
  Output<abi::Reply> reply;
  Wait([&] {
    const auto status = function(&owner, &command, &reply.value);
    reply.Check();
    if (status == abi::kBusy) return false;
    if (status != expected || reply.value.status != expected ||
        reply.value.version != abi::kVersion || reply.value.size != sizeof(abi::Reply)) {
      std::fprintf(stderr, "opcode=%u status=%u reply_status=%u version=%u size=%u expected=%u\n",
          command.opcode, status, reply.value.status, reply.value.version, reply.value.size, expected);
    }
    assert(status == expected && reply.value.status == expected &&
        reply.value.version == abi::kVersion && reply.value.size == sizeof(abi::Reply));
    return true;
  });
  return reply.value;
}

abi::Command Ack(const abi::ActionLease& lease, std::uint32_t result) {
  auto command = Request(abi::kAckAction);
  command.args[0] = lease.owner;
  command.args[1] = lease.context;
  command.args[2] = lease.effect;
  command.args[3] = lease.action;
  command.args[4] = lease.target;
  command.args[5] = result;
  return command;
}
}  // namespace

int wmain(int argc, wchar_t** argv) {
  assert(argc == 2 || argc == 3);
  sdk::BootstrapHost host;
  std::array<std::uint8_t, 16> nonce{};
  nonce.fill(7);
  const auto diagnostic = host.Preflight(argv[1], true, nonce);
  if (argc == 3 && std::wcscmp(argv[2], L"--missing") == 0) {
    assert(diagnostic.status == sdk::Status::kMissingExports);
    std::printf("legacy_load=1 rejected_missing_abi3_exports=1\n");
    return 0;
  }
  assert(diagnostic.status == sdk::Status::kOk && diagnostic.core_abi == 3 &&
      diagnostic.core_exports == 11 && diagnostic.rinf_exports == 4 && diagnostic.owners == 0);
  // 没有 SDK nonce 回包或 Verify，不签发 ParentVerifiedSameSdk。
  assert(host.verified_core_module() == nullptr);
  const auto module = GetModuleHandleW(host.compiled_lib_path().c_str());
  const auto open = Resolve<abi::OpenFn>(module, abi::kOpenSymbol);
  const auto key = Resolve<abi::KeyFn>(module, abi::kKeySymbol);
  const auto revoke = Resolve<abi::RevokeFn>(module, abi::kRevokeSymbol);
  const auto join = Resolve<abi::JoinFn>(module, abi::kJoinSymbol);
  const auto inspect = Resolve<abi::InspectFn>(module, abi::kInspectSymbol);
  const auto mode = Resolve<abi::PeerModeFn>(module, abi::kPeerModeSymbol);
  const auto read = Resolve<abi::PeerReadFn>(module, abi::kPeerReadSymbol);
  const auto support = Resolve<abi::SupportFn>(module, abi::kSupportSymbol);
  const auto command = Resolve<abi::CommandFn>(module, abi::kCommandSymbol);
  const auto copy = Resolve<abi::CopyFn>(module, abi::kCopySymbol);
  const auto projection = Resolve<abi::ProjectionFn>(module, abi::kProjectionSymbol);
  Output<abi::SpikeSnapshot> snapshot;
  assert(inspect(nullptr, &snapshot.value) == abi::kDisabled);
  snapshot.Check();
  assert(snapshot.value.version == 3 && snapshot.value.size == 176);

  static const std::uint8_t context = 1;  // 进程寿命固定 fixture 对象。
  abi::SpikeOpen request{abi::kVersion, sizeof(request), 0, 0, &context,
      reinterpret_cast<const std::uint8_t*>(abi::kFixtureName), sizeof(abi::kFixtureName) - 1,
      abi::kDeclaredSynthetic, abi::kFixturePlatform, 1};
  Output<abi::SpikeOwner> owner;
  assert(open(&request, &owner.value) == abi::kDisabled && !owner.value.handle);
  request.explicit_opt_in = 1;
  assert(open(&request, &owner.value) == abi::kOk);
  owner.Check();
  const auto cap = owner.value;
  Wait([&] { return inspect(&cap, &snapshot.value) == abi::kOk; });
  snapshot.Check();
  assert(snapshot.value.source_origin == abi::kDeclaredSynthetic &&
      snapshot.value.scope_delta != 0 && snapshot.value.profile_revision != 0 &&
      snapshot.value.cell_width != 0 && snapshot.value.cell_height != 0);
  assert(support(&cap, 1) == abi::kOk && support(&cap, 2) == abi::kOk &&
      support(&cap, 3) == abi::kOk && support(&cap, 4) == abi::kUnsupported &&
      support(&cap, 5) == abi::kProductionNotReady &&
      support(&cap, 6) == abi::kScopedTransportUnavailable);

  abi::SpikeKey input{};
  input.version = 3;
  input.size = sizeof(input);
  input.physical = input.native_event = 1;
  input.layout_revision = 1;
  input.raw_source = abi::kRawVerified;
  input.raw_kind = input.code_kind = input.legacy_kind = abi::kFunction;
  input.raw_value = input.code_value = input.legacy_value = 4;
  input.dual_source = input.alt_source = 1;
  input.text_ownership = abi::kTerminalKey;
  Output<abi::SpikeAdmission> admission;
  const auto initial_seq = snapshot.value.next_seq;
  unsigned busy_down = 0;
  Wait([&] {
    input.phase = abi::kDown;
    const auto status = key(&cap, &input, &admission.value);
    admission.Check();
    if (status == abi::kOk) return true;
    assert(status == abi::kBusy && admission.value.status == status &&
        admission.value.seq == 0 && admission.value.effect == 0 &&
        admission.value.action == 0 && admission.value.byte_len == 0);
    ++busy_down;
    // Busy Down 已登记拒绝 press；原 Repeat 不复活，原 Up 完成后才开始独立 press。
    for (auto phase : {abi::kRepeat, abi::kUp}) {
      input.phase = phase;
      ++input.native_event;
      assert(key(&cap, &input, &admission.value) == abi::kInvalid);
      admission.Check();
      assert(admission.value.status == abi::kInvalid && admission.value.seq == 0 &&
          admission.value.effect == 0 && admission.value.action == 0 && admission.value.byte_len == 0);
    }
    Wait([&] {
      const auto inspected = inspect(&cap, &snapshot.value);
      snapshot.Check();
      assert(inspected == abi::kOk || inspected == abi::kBusy);
      if (inspected == abi::kBusy) return false;
      assert(snapshot.value.next_seq == initial_seq);
      return snapshot.value.charged_slots == 0 && snapshot.value.inflight == 0;
    });
    ++input.native_event;
    return false;
  });
  assert(admission.value.status == abi::kOk && admission.value.version == 3 &&
       admission.value.size == 80 && admission.value.effect != 0 &&
       admission.value.ticket.owner == 0 && admission.value.seq == initial_seq);
  input.phase = abi::kUp;
  ++input.native_event;
  const auto up_status = key(&cap, &input, &admission.value);
  admission.Check();
  assert(admission.value.status == up_status);
  if (up_status == abi::kBusy) {
    assert(admission.value.seq == 0 && admission.value.effect == 0 &&
        admission.value.action == 0 && admission.value.byte_len == 0);
  } else {
    if (up_status != abi::kOk || admission.value.seq != initial_seq + 1) {
      std::fprintf(stderr, "up_status=%u up_seq=%llu initial_seq=%llu busy_down=%u\n",
          up_status, admission.value.seq, initial_seq, busy_down);
    }
    assert(up_status == abi::kOk && admission.value.seq == initial_seq + 1);
  }
  // Up Busy 已登记 PendingUp；只等 worker 分配原 Up 的唯一 seq，不重发输入。
  Wait([&] {
    const auto inspected = inspect(&cap, &snapshot.value);
    snapshot.Check();
    assert(inspected == abi::kOk || inspected == abi::kBusy);
    if (inspected == abi::kBusy) return false;
    assert(snapshot.value.next_seq == initial_seq + 1 || snapshot.value.next_seq == initial_seq + 2);
    return snapshot.value.next_seq == initial_seq + 2;
  });
  std::array<std::uint8_t, 128> bytes{};
  std::size_t received = 0;
  Wait([&] {
    std::size_t count = 0;
    const auto status = read(&cap, bytes.data() + received, bytes.size() - received, &count);
    assert(status == abi::kOk || status == abi::kBusy);
    received += count;
    return received == 4;
  });
  assert(std::memcmp(bytes.data(), "\x1b[3~", 4) == 0);
  Wait([&] { return inspect(&cap, &snapshot.value) == abi::kOk && snapshot.value.charged_slots == 0; });
  snapshot.Check();
  assert(snapshot.value.next_seq == initial_seq + 2);
  std::size_t duplicate_count = 0;
  assert(read(&cap, bytes.data(), bytes.size(), &duplicate_count) == abi::kBusy && duplicate_count == 0);
  std::printf("key_press=1 busy_down=%u pending_up=%u admitted_seqs=2 duplicate_bytes=0\n",
      busy_down, up_status == abi::kBusy ? 1U : 0U);

  auto begin = Request(abi::kIndependentText);
  begin.args[0] = reinterpret_cast<std::uintptr_t>(cap.native_context);
  begin.args[1] = 1;
  const auto ticket = Call(command, cap, begin).ticket;
  assert(ticket.owner == cap.handle && ticket.context == begin.args[0] &&
      ticket.generation != 0 && ticket.transaction == ticket.effect && ticket.effect != 0);
  constexpr char text[] = "ABI3\xE8\xAF\xAD\xE4\xB9\x89";
  auto preedit = Request(abi::kPreedit);
  preedit.ticket = ticket;
  preedit.args[0] = 1;
  preedit.bytes = reinterpret_cast<const std::uint8_t*>(text);
  preedit.length = sizeof(text) - 1;
  Call(command, cap, preedit);
  auto grant_request = Request(abi::kGrantPayload);
  grant_request.ticket = ticket;
  grant_request.args[0] = sizeof(text) - 1;
  const auto grant = Call(command, cap, grant_request).grant;
  assert(grant.id != 0 && grant.total == sizeof(text) - 1 && grant.chunk == 32);
  auto chunk = Request(abi::kPayloadChunk);
  chunk.ticket = ticket;
  chunk.grant = grant;
  chunk.flags = 1;
  chunk.bytes = preedit.bytes;
  chunk.length = preedit.length;
  Call(command, cap, chunk);
  auto finish = Request(abi::kFinishPayload);
  finish.ticket = ticket;
  finish.args[0] = 2;
  assert(Call(command, cap, finish).kind == 1);
  received = 0;
  Wait([&] {
    std::size_t count = 0;
    const auto status = read(&cap, bytes.data() + received, bytes.size() - received, &count);
    assert(status == abi::kOk || status == abi::kBusy);
    received += count;
    return received == sizeof(text) - 1;
  });
  assert(std::memcmp(bytes.data(), text, received) == 0);

  Wait([&] { return mode(&cap, 15) == abi::kOk; });
  Output<abi::Projection> frame;
  Wait([&] { return projection(&cap, &frame.value) == abi::kOk; });
  frame.Check();
  assert(frame.value.version == 3 && frame.value.size == 928 &&
      frame.value.owner == cap.handle && frame.value.source_generation == cap.generation &&
      frame.value.frame != 0 && frame.value.count <= 36 && frame.value.count != 0 &&
      frame.value.cells[0].scalar == 'h' && frame.value.cells[0].width == 1);
  auto select = Request(abi::kControlEffect);
  select.args[0] = 3;
  select.args[4] = 5;
  Call(command, cap, select);
  auto copy_request = Request(abi::kControlEffect);
  copy_request.args[0] = 4;
  Call(command, cap, copy_request);
  const auto lease = Call(command, cap, Request(abi::kClaimAction)).action;
  assert(lease.owner == cap.handle && lease.context == begin.args[0] && lease.action == 11);
  std::array<std::uint8_t, 130> copied{};
  copied.front() = copied.back() = 0xa5;
  std::size_t count = 0;
  Wait([&] { return copy(&cap, &lease, copied.data() + 1, 128, &count) == abi::kOk; });
  assert(count == 5 && std::memcmp(copied.data() + 1, "hello", 5) == 0 &&
      copied.front() == 0xa5 && copied.back() == 0xa5);
  Call(command, cap, Ack(lease, 0));
  auto forged = cap;
  ++forged.generation;
  abi::Reply untouched{};
  const auto inspect_request = Request(abi::kInspectEffects);
  assert(command(&forged, &inspect_request, &untouched) == abi::kInvalidSource);

  Call(command, cap, copy_request);
  const auto late_lease = Call(command, cap, Request(abi::kClaimAction)).action;
  assert(revoke(&cap) == abi::kOk && join(&cap) == abi::kInvalidSource);
  auto late = Request(abi::kIndependentText);
  assert(command(&cap, &late, &untouched) == abi::kStopped);
  count = 127;
  copied.fill(0xa5);
  Wait([&] {
    const auto status = copy(&cap, &late_lease, copied.data() + 1, 128, &count);
    assert(count == 127);
    for (auto byte : copied) assert(byte == 0xa5);
    assert(status == abi::kBusy || status == abi::kStopped);
    return status == abi::kStopped;
  });
  assert(projection(&cap, &frame.value) == abi::kStopped);
  Call(command, cap, Ack(late_lease, 2));
  auto retired = Request(abi::kConfirmRetired);
  retired.args[0] = begin.args[0];
  Call(command, cap, retired);
  std::uint32_t joined = abi::kInvalid;
  std::thread management([&] { joined = join(&cap); });
  management.join();
  assert(joined == abi::kOk);
  Wait([&] { return inspect(&cap, &snapshot.value) == abi::kOk; });
  snapshot.Check();
  assert(snapshot.value.stopped == 1 && snapshot.value.worker_done == 1 &&
      snapshot.value.worker_joined == 1 && snapshot.value.native_retired == 1 &&
      snapshot.value.core_revoked == 1 && snapshot.value.charged_slots == 0 &&
      snapshot.value.inflight == 0 && snapshot.value.retained_bytes == 0);
  std::printf("actual_rust=1 abi=3 mapped_exports=11 synthetic_only=1 sizes=48,56,96,80,176,136,120,928 payload=1 copy=1 projection=1 worker_joined=1 late_callbacks_closed=1\n");
  return 0;
}
