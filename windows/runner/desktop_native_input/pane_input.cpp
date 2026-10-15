#include "host_internal.h"
#include <windowsx.h>
#include <algorithm>

namespace guosshell::desktop_native_input {
namespace {
uint32_t Modifiers(const std::array<BYTE, 256>& state) noexcept {
  return (state[VK_SHIFT] ? 1u : 0u) | (state[VK_CONTROL] ? 2u : 0u) | (state[VK_MENU] ? 4u : 0u) |
      ((state[VK_LWIN] || state[VK_RWIN]) ? 8u : 0u) | (state[VK_LMENU] ? 16u : 0u) | (state[VK_RMENU] ? 32u : 0u);
}
bool Code(ni::ScalarMetadata value, uint32_t* kind, uint32_t* code) noexcept {
  if (value.kind == ni::MetadataKind::Scalar || value.kind == ni::MetadataKind::Dead) {
    if (value.scalar > 0x10ffff || (value.scalar >= 0xd800 && value.scalar <= 0xdfff) || !value.scalar) return false;
    *kind = 1; *code = value.scalar; return true;
  }
  if (value.kind != ni::MetadataKind::Named) return false;
  *code = 0;
  switch (value.scalar) {
    case VK_SPACE: *kind = 1; *code = ' '; return true;
    case VK_RETURN: *kind = 2; break; case VK_ESCAPE: *kind = 3; break;
    case VK_TAB: *kind = 4; break; case VK_BACK: *kind = 5; break;
    case VK_DELETE: *kind = 6; break; case VK_INSERT: *kind = 7; break;
    case VK_HOME: *kind = 8; break; case VK_END: *kind = 9; break;
    case VK_PRIOR: *kind = 10; break; case VK_NEXT: *kind = 11; break;
    case VK_UP: *kind = 12; break; case VK_DOWN: *kind = 13; break;
    case VK_LEFT: *kind = 14; break; case VK_RIGHT: *kind = 15; break;
    default:
      if (value.scalar < VK_F1 || value.scalar > VK_F24) return false;
      *kind = 16; *code = value.scalar - VK_F1 + 1; break;
  }
  return true;
}
uint32_t Candidate(const GuosshNormalKey& key) noexcept {
  // 只提出工作区默认候选；应用/profile 覆盖、消费与 Repeat 仍由 Rust matcher 决定。
  const uint32_t mods = key.modifiers & 15;
  if ((mods & (2u | 4u | 8u)) != 2u) return 0;
  if (key.raw_kind == 4) return mods & 1 ? 6 : 5;
  if (key.raw_kind != 1) return 0;
  uint32_t c = key.raw_value;
  if (c >= 'A' && c <= 'Z') c += 'a' - 'A';
  if (!(mods & 1)) {
    if (c == ',') return 9;
    if (c == '=' || c == '+') return 14;
    if (c == '-') return 15;
    if (c == '0') return 16;
    return 0;
  }
  switch (c) {
    case 't': return 1; case 'd': return 2; case 'e': return 3; case 'w': return 4;
    case 'p': return 7; case 'f': return 8; case 'b': return 10;
    case 'c': return 11; case 'v': return 12; case 'a': return 13; case '=': case '+': return 14;
    default: return 0;
  }
}
ni::ScalarMetadata Metadata(const ni::LayoutCell& cell) noexcept {
  auto metadata = cell.result;
  // flags=4 返回的 spacing dead 字符只用于键身份；不是已提交文本。
  if (metadata.kind == ni::MetadataKind::Dead && cell.units[0] && !ni::HighSurrogate(cell.units[0]) && !ni::LowSurrogate(cell.units[0]))
    metadata.scalar = cell.units[0];
  return metadata;
}
}
void PaneRoot::Raw(const MSG& message) {
  bool cleaned = false;
  try {
  Poll();
  if (!Active() || !surface_) {
    DefWindowProcW(message.hwnd, message.message, message.wParam, message.lParam);
    cleaned = true;
    if (Active()) Fail("CONTEXT_ADMISSION_BUSY");
    return;
  }
  bool altgr_layout = false;
  if (layout_) for (size_t scan = 1; scan < 256; ++scan) {
    const auto& plain = layout_->Cell(scan, 0).result; const auto& altgr = layout_->Cell(scan, 4).result;
    if (altgr.kind == ni::MetadataKind::Scalar && (plain.kind != altgr.kind || plain.scalar != altgr.scalar)) { altgr_layout = true; break; }
  }
  ni::NativeKey key{};
  const auto status = host_.raw.Read(message, raw_epoch_, surface_->Profile(), altgr_layout, &key);
  cleaned = status != RawStatus::Ignored && status != RawStatus::WrongThread;
  if (!cleaned) { DefWindowProcW(message.hwnd, message.message, message.wParam, message.lParam); cleaned = true; }
  if (status != RawStatus::Ready) {
    Fail(status == RawStatus::InitialStateUnknown ? "RAW_INITIAL_STATE_UNVERIFIED" : "RAW_SOURCE_REJECTED");
    return;
  }
  if (!host_.raw.Current(key) || !Active()) return;
  physical_state_ = key.physical_state;
  if (ni::NamedKey(static_cast<UINT>(key.message.wParam))) {
    key.raw = key.legacy = {ni::MetadataKind::Named, static_cast<uint32_t>(key.message.wParam)};
    key.trusted_raw = true; key.metadata_fault = ni::Fault::None;
  } else if (layout_ &&
      ni::SameProfile(layout_->Profile(), surface_->Profile()) && !key.e1) {
    const size_t scan = key.scan + (key.extended ? 256 : 0);
    const size_t level = (key.physical_state[VK_SHIFT] ? 1u : 0u) | ((key.queue_state[VK_CAPITAL] & 1) ? 2u : 0u) |
        (altgr_layout && key.physical_state[VK_RMENU] ? 4u : 0u);
    const auto& base = layout_->Cell(scan, 0); const auto& legacy = layout_->Cell(scan, level);
    if (base.vk == key.message.wParam && legacy.vk == base.vk) {
      key.raw = Metadata(base); key.legacy = Metadata(legacy);
      uint32_t kind{}, value{};
      key.trusted_raw = Code(key.raw, &kind, &value) && Code(key.legacy, &kind, &value);
      if (key.trusted_raw) key.metadata_fault = ni::Fault::None;
    }
  }
  if (surface_->State() != ni::FocusState::Active) {
    if (key.up) Gate(surface_->Owner(), key, surface_->Store()->OriginalTicket());
    else Fail("CONTEXT_ADMISSION_BUSY");
    Poll();
    return;
  }
  const auto result = surface_->DispatchCanonical(key, nullptr);
  if (result.fault != ni::Fault::None) Fail("NATIVE_TEXT_ROUTE_REJECTED");
  Poll();
  } catch (...) {
    if (!cleaned) DefWindowProcW(message.hwnd, message.message, message.wParam, message.lParam);
    active_ = false; revoking_ = true; failed_ = true;
    if (!deadline_) deadline_ = GetTickCount64() + kTransitionMilliseconds;
  }
}
ni::GateReceipt PaneRoot::Gate(const ni::OwnerRef& owner, const ni::NativeKey& key, const ni::TicketRef* original) noexcept {
  ni::GateReceipt receipt{};
  if (!Active() || !surface_ || owner.context != surface_->Owner().context || !host_.raw.Current(key)) return receipt;
  auto request = Abi<GuosshNormalKey>();
  request.physical = key.physical; request.phase = key.up ? 3 : key.repeat ? 2 : 1;
  request.modifiers = Modifiers(key.physical_state);
  request.context_id = owner.context;
  request.resume_text_ticket = original && !surface_->Store()->Sealed() ? original->transaction : 0;
  const bool codes = key.trusted_raw && Code(key.raw, &request.raw_kind, &request.raw_value) &&
      Code(key.legacy, &request.code_kind, &request.code_value);
  request.legacy_kind = request.code_kind; request.legacy_value = request.code_value;
  request.raw_known = request.same_event_dual = codes ? 1 : 0;
  // Raw Ctrl+RightAlt 本身不能区分真实 Ctrl 与平台生成的 AltGr 兼容 Ctrl。
  // 不按时间/FIFO 猜配，也不把该歧义送成普通 Ctrl+Alt 字节。
  request.alt_provenance = (request.modifiers & 32) && (request.modifiers & 2) ? 0 : 1;
  const bool altgr = (request.modifiers & 32) && !(request.modifiers & 2) && (key.queue_state[VK_CONTROL] & 0x80);
  const bool text_modifiers = !(request.modifiers & (2 | 8 | 16)) && (!(request.modifiers & 4) || altgr);
  const bool native_text = (request.code_kind == 1 && text_modifiers) || request.resume_text_ticket;
  request.text_ownership = native_text ? 2 : 1;
  request.default_action = Candidate(request);
  auto context = Context(); auto admission = Abi<GuosshNormalAdmission>();
  const auto status = host_.api.key(&context, &request, &admission);
  if (status != GUOSSH_NORMAL_OK) {
    // Rust 已对拒绝包执行原 Press 锁存；不调用 TSF，不自行重签 ticket。
    if (status == GUOSSH_NORMAL_BUSY) {
      try { Fail("NATIVE_KEY_BUSY"); } catch (...) { active_ = false; revoking_ = true; }
    }
    return receipt;
  }
  if (admission.text_op == 3) {
    ending_.context_id = admission.end_context_id; ending_.transaction_id = admission.end_transaction_id;
    ending_.end_fence = admission.end_fence;
  }
  receipt.disposition = admission.route == 1 ? ni::Disposition::TerminalDeferred :
      admission.route == 2 ? ni::Disposition::AllowNativeText : ni::Disposition::ConsumedDeferred;
  receipt.ticket = {generation_, owner.context, admission.text_ticket, admission.effect};
  receipt.trusted_raw = codes;
  // NativeText Up 只在原 Open store/ticket 中发送；Rust 对 sealed/旧 context 返回 Consumed。
  receipt.lifecycle_only = key.up && (!original || surface_->Store()->Sealed() ||
      admission.text_ticket != original->transaction || admission.text_op != 0);
  return receipt;
}
bool PaneRoot::Ticket(const ni::TicketRef& ticket) const noexcept {
  return Active() && surface_ && ticket.owner_generation == generation_ && ticket.context == surface_->Owner().context &&
      surface_->Store() && surface_->Store()->OriginalTicket() && ticket == *surface_->Store()->OriginalTicket();
}
bool PaneRoot::ReservePayload(const ni::TicketRef& ticket, uint32_t bound, ni::PayloadGrant* grant) noexcept {
  if (!Ticket(ticket) || bound > ni::kTextBytes) return false;
  // NativeMemory 已在 store 分配前持有固定额度；Rust 的精确 UTF-8 长度在 PreparePayload 授额。
  *grant = {ticket.transaction, bound, 4096};
  return true;
}
bool PaneRoot::PreparePayload(const ni::TicketRef& ticket, uint32_t bytes) noexcept {
  if (!Ticket(ticket) || bytes > ni::kTextBytes) return false;
  auto context = Context(); auto request = Abi<GuosshNormalTextReservation>();
  request.text_ticket = ticket.transaction; request.utf8_bytes = bytes;
  return host_.api.reserve_payload(&context, &request) == GUOSSH_NORMAL_OK;
}
void PaneRoot::EmitComposition(bool ended) {
  if (!surface_ || (ended && !composition_ticket_) || (!ended && !geometry_.valid)) return;
  Map message{{Value("version"), Value(int32_t{1})}, {Value("paneToken"), Value(token_)},
      {Value("frameRevision"), Value(static_cast<int64_t>(projection_.frame_revision))}, {Value("acpStart"), Value(int32_t{0})},
      {Value("text"), Value(ended ? std::string{} : Utf8(composition_.data(), composition_.size()))},
      {Value("state"), Value(ended ? "ended" : "open")}};
  Put(message, "contextId", surface_->Owner().context); Put(message, "transactionId", composition_ticket_);
  Put(message, "editRevision", composition_revision_);
  // 原身份已编码；先消费待结束标记，通知重入不得再次结束同一事务。
  if (ended) composition_ticket_ = composition_revision_ = 0;
  host_.Emit("compositionLayout", std::move(message));
}
bool PaneRoot::Preedit(const ni::TicketRef& ticket, uint64_t revision, const WCHAR* text, ULONG count, LONG, LONG) noexcept {
  try {
    if (!Ticket(ticket) || count > ni::kTextUnits) return false;
    auto bytes = Utf8(text, count);
    if (count && bytes.empty()) return false;
    auto context = Context(); auto request = Abi<GuosshNormalComposition>();
    request.text_ticket = ticket.transaction; request.kind = 1;
    request.text = reinterpret_cast<const uint8_t*>(bytes.data()); request.text_len = bytes.size();
    if (host_.api.composition(&context, &request) != GUOSSH_NORMAL_OK) return false;
    composition_.assign(text, count); composition_revision_ = revision; composition_ticket_ = ticket.transaction;
    EmitComposition();
    return Active();
  } catch (...) { return false; }
}
bool PaneRoot::Payload(const ni::TicketRef& ticket, const ni::PayloadGrant& grant, uint32_t offset,
    const char* bytes, uint32_t count, bool) noexcept {
  if (!Ticket(ticket) || grant.id != ticket.transaction || count > grant.bytes || offset > grant.bytes - count) return false;
  auto context = Context(); auto request = Abi<GuosshNormalTextChunk>();
  request.text_ticket = ticket.transaction; request.offset = offset;
  request.bytes = reinterpret_cast<const uint8_t*>(bytes); request.len = count;
  return host_.api.payload(&context, &request) == GUOSSH_NORMAL_OK;
}
bool PaneRoot::Finish(const ni::TicketRef& ticket, uint64_t) noexcept {
  if (!Ticket(ticket)) return false;
  auto context = Context(); auto request = Abi<GuosshNormalTextFinish>(); request.text_ticket = ticket.transaction;
  if (host_.api.finish(&context, &request) != GUOSSH_NORMAL_OK) return false;
  try { if (composition_ticket_) EmitComposition(true); composition_.clear(); } catch (...) {}
  return true;
}
void PaneRoot::Cancel(const ni::TicketRef& ticket, ni::Fault) noexcept {
  if (!bound_.handle || revoking_ || ending_.end_fence) return;
  if (ticket.owner_generation != generation_ || !surface_ || ticket.context != surface_->Owner().context) return;
  // 失败也先 seal 原 root，Rust 进入 Ending；只有真实 quiet 的原 fence ACK 才能 Cancelled。
  try { Seal("TEXT_TRANSACTION_CANCELLED"); } catch (...) { active_ = false; revoking_ = true; }
}
void PaneRoot::Faulted(const ni::OwnerRef&, ni::Fault) noexcept {
  try { Fail("NATIVE_CONTEXT_FAULT"); } catch (...) { active_ = false; revoking_ = true; }
}
void PaneRoot::PointerMessage(const MSG& message) {
  if (!Active() || !geometry_.valid || GetFocus() != hwnd_ || native_request_ == UINT64_MAX) return;
  if (GetForegroundWindow() != GetAncestor(host_.view, GA_ROOT)) { Seal(); return; }
  POINT point{GET_X_LPARAM(message.lParam), GET_Y_LPARAM(message.lParam)};
  const bool wheel = message.message == WM_MOUSEWHEEL || message.message == WM_MOUSEHWHEEL;
  if (!wheel) ClientToScreen(hwnd_, &point);
  ScreenToClient(host_.view, &point);
  double local_x{}, local_y{};
  if (!geometry_.Local(point.x / geometry_.dpr, point.y / geometry_.dpr, &local_x, &local_y)) {
    if (pointer_capture_ && (message.message == WM_LBUTTONUP || message.message == WM_MBUTTONUP || message.message == WM_RBUTTONUP)) {
      ReleasePointerCapture(); Fail("POINTER_LEFT_PRESENTED_REGION");
    }
    return;
  }
  const double column = std::floor(local_x / geometry_.cell_width);
  const double row = std::floor((local_y + std::fmod(geometry_.scroll_offset, geometry_.cell_height)) / geometry_.cell_height);
  if (column < 0 || row < 0 || column > UINT32_MAX || row > UINT32_MAX ||
      projection_.viewport_top > INT64_MAX - static_cast<int64_t>(row)) return;
  auto request = Abi<GuosshNormalPointer>(); request.native_request_id = native_request_ + 1;
  request.input.geometry = geometry_.native;
  request.input.column = request.input.remote_column = static_cast<uint32_t>(column);
  request.input.remote_row = static_cast<uint32_t>(row);
  request.input.stable_hit_row = projection_.viewport_top + static_cast<int64_t>(row);
  const auto flags = GET_KEYSTATE_WPARAM(message.wParam);
  request.input.buttons = (flags & MK_LBUTTON ? 1u : 0u) | (flags & MK_MBUTTON ? 2u : 0u) | (flags & MK_RBUTTON ? 4u : 0u);
  request.input.modifiers = (flags & MK_SHIFT ? 1u : 0u) | (flags & MK_CONTROL ? 2u : 0u) | (Modifiers(physical_state_) & 12u);
  switch (message.message) {
    case WM_LBUTTONDOWN: case WM_LBUTTONDBLCLK: request.input.kind = 1; request.input.button = 1; break;
    case WM_MBUTTONDOWN: case WM_MBUTTONDBLCLK: request.input.kind = 1; request.input.button = 2; break;
    case WM_RBUTTONDOWN: case WM_RBUTTONDBLCLK: request.input.kind = 1; request.input.button = 3; break;
    case WM_LBUTTONUP: request.input.kind = 3; request.input.button = 1; break;
    case WM_MBUTTONUP: request.input.kind = 3; request.input.button = 2; break;
    case WM_RBUTTONUP: request.input.kind = 3; request.input.button = 3; break;
    case WM_MOUSEMOVE: request.input.kind = 2; break;
    case WM_MOUSEWHEEL: {
      request.input.kind = 4;
      wheel_remainder_ += GET_WHEEL_DELTA_WPARAM(message.wParam);
      const int steps = wheel_remainder_ / WHEEL_DELTA;
      if (!steps) return;
      wheel_remainder_ %= WHEEL_DELTA;
      UINT lines = 3;
      SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &lines, 0);
      if (lines == WHEEL_PAGESCROLL) lines = static_cast<UINT>(geometry_.local_height / geometry_.cell_height);
      const int64_t delta = -static_cast<int64_t>(steps) * lines;
      if (!delta) return;
      if (delta < -64 || delta > 64) { Fail("WHEEL_RANGE_UNSUPPORTED"); return; }
      request.input.delta_y = static_cast<int32_t>(delta);
      break;
    }
    case WM_MOUSEHWHEEL: {
      horizontal_wheel_remainder_ += GET_WHEEL_DELTA_WPARAM(message.wParam);
      const int steps = horizontal_wheel_remainder_ / WHEEL_DELTA;
      horizontal_wheel_remainder_ %= WHEEL_DELTA;
      if (!steps) return;
      UINT characters{};
      if (!SystemParametersInfoW(SPI_GETWHEELSCROLLCHARS, 0, &characters, 0)) { Fail("WHEEL_SETTINGS_UNAVAILABLE"); return; }
      if (characters == WHEEL_PAGESCROLL) characters = static_cast<UINT>(std::max(1.0, std::floor(geometry_.local_width / geometry_.cell_width)));
      const int64_t delta = static_cast<int64_t>(steps) * characters;
      if (!delta) return;
      if (delta < -64 || delta > 64) { Fail("WHEEL_RANGE_UNSUPPORTED"); return; }
      request.input.kind = 4;
      request.input.delta_x = static_cast<int32_t>(delta);
      break;
    }
    default: return;
  }
  auto context = Context(); auto admission = Abi<GuosshNormalAdmission>();
  const auto status = host_.api.pointer(&context, &request, &admission);
  if (status != GUOSSH_NORMAL_OK) { Fail(status == GUOSSH_NORMAL_BUSY ? "NATIVE_POINTER_BUSY" : "NATIVE_POINTER_REJECTED"); return; }
  ++native_request_;
  if (request.input.kind == 1) {
    SetCapture(hwnd_); pointer_capture_ = GetCapture() == hwnd_;
    if (!pointer_capture_) { Fail("POINTER_CAPTURE_UNAVAILABLE"); return; }
  }
  if (request.input.kind == 3 && request.input.buttons == 0) ReleasePointerCapture();
  if (admission.text_op == 3) {
    ending_.context_id = admission.end_context_id; ending_.transaction_id = admission.end_transaction_id; ending_.end_fence = admission.end_fence;
  }
  Poll();
}
}  // namespace guosshell::desktop_native_input
