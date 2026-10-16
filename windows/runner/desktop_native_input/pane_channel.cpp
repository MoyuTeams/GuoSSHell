#include "host_internal.h"
#include <algorithm>
#include <array>

namespace guosshell::desktop_native_input {
void PaneRoot::ApplyGeometry() {
  if (surface_) surface_->SetGeometry(geometry_.TextGeometry(host_.view, projection_, id_));
}
bool PaneRoot::Invalidate() {
  geometry_.valid = false;
  ApplyGeometry();
  if (pointer_capture_) { Fail("POINTER_CAPTURE_GEOMETRY_INVALIDATED"); return false; }
  if (!bound_.handle || !Active()) return true;
  if (native_request_ == UINT64_MAX) { Fail("NATIVE_COUNTER_EXHAUSTED"); return false; }
  auto context = Context();
  const auto status = host_.api.invalidate_geometry(&context, native_request_ + 1);
  if (status == GUOSSH_NORMAL_OK) { ++native_request_; return true; }
  // Busy 选择立即 seal，不分配新 ID，也不让后续 geometry/key 越过原 pointer。
  Fail(status == GUOSSH_NORMAL_BUSY ? "NATIVE_ORDER_BUSY" : "GEOMETRY_INVALIDATION_REJECTED");
  return false;
}
Map PaneRoot::Present(const Map& args) {
  Geometry geometry{}; uint64_t attempt{}, epoch{};
  if (!Active() || !Geometry::Parse(args, &geometry) || !U64(args, "attempt", &attempt) || attempt != attempt_ ||
      !U64(args, "focusEpoch", &epoch) || epoch != bound_.focus_epoch || native_request_ == UINT64_MAX) return Reply("failed");
  auto context = Context(); auto projection = Abi<GuosshNormalProjection>();
  if (host_.api.projection(&context, &projection) != GUOSSH_NORMAL_OK || !geometry.Matches(args, projection)) return Reply("failed");
  geometry.native.frame_revision = projection.frame_revision;
  geometry.native.row_revision = projection.row_revision;
  geometry.native.viewport_top = projection.viewport_top;
  opening_ = true;
  const bool positioned = geometry.Position(hwnd_, host_.view);
  opening_ = false;
  if (!positioned) { Fail("INPUT_REGION_UNAVAILABLE"); return Reply("failed"); }
  auto request = Abi<GuosshNormalPresentedGeometry>();
  request.native_request_id = native_request_ + 1; request.geometry = geometry.native;
  const auto status = host_.api.presented_geometry(&context, &request);
  if (status != GUOSSH_NORMAL_OK) { Fail(status == GUOSSH_NORMAL_BUSY ? "NATIVE_ORDER_BUSY" : "PRESENTATION_REJECTED"); return Reply("failed"); }
  ++native_request_;
  geometry.valid = true; geometry_ = geometry; projection_ = projection;
  ApplyGeometry();
  return Reply("presented");
}
Map PaneRoot::TextEnd(const Map& args) {
  uint64_t context{}, transaction{}, fence{};
  if (!U64(args, "contextId", &context) || !U64(args, "transactionId", &transaction) ||
      !U64(args, "endFence", &fence) || !fence) return Reply("failed");
  if (!QueryEnd()) return Reply("pending");
  for (const auto& ended : ended_)
    if (ended.context_id == context && ended.transaction_id == transaction && ended.end_fence == fence) return Reply("nativequiet");
  if (ending_.context_id != context || ending_.transaction_id != transaction || ending_.end_fence != fence) return Reply("failed");
  Poll();
  return Reply(!ending_.end_fence ? "nativequiet" : failed_ ? "failed" : "accepted");
}
Map PaneRoot::TextLayout(const Map& args) {
  uint64_t context{}, ticket{}, revision{}; int64_t frame{}, start{};
  if (!Active() || !surface_ || !geometry_.valid || !U64(args, "contextId", &context) || context != surface_->Owner().context ||
      !U64(args, "transactionId", &ticket) || !ticket || ticket != composition_ticket_ || !U64(args, "editRevision", &revision) ||
      revision != composition_revision_ || !Integer(args, "frameRevision", &frame) || frame != projection_.frame_revision ||
      !Integer(args, "acpStart", &start) || start != 0 || composition_.size() > ni::kTextUnits) return Reply("failed");
  Geometry layout{};
  // composition width/height 是 TextPainter 尺寸；终端 transform/clip 仍须精确匹配已呈现布局。
  Map terminal(args); terminal[Value("width")] = Value(geometry_.width); terminal[Value("height")] = Value(geometry_.height);
  if (!Geometry::Parse(terminal, &layout)) return Reply("failed");
  if (layout.native.font_revision != geometry_.native.font_revision || layout.native.layout_revision != geometry_.native.layout_revision ||
      layout.native.transform_revision != geometry_.native.transform_revision || layout.viewport_revision != geometry_.viewport_revision ||
      layout.native.clip_x != geometry_.native.clip_x || layout.native.clip_y != geometry_.native.clip_y ||
      layout.native.clip_width != geometry_.native.clip_width || layout.native.clip_height != geometry_.native.clip_height ||
      layout.cell_width != geometry_.cell_width || layout.cell_height != geometry_.cell_height || layout.scroll_offset != geometry_.scroll_offset ||
      layout.dpr != geometry_.dpr || !std::equal(std::begin(layout.native.transform), std::end(layout.native.transform), std::begin(geometry_.native.transform))) return Reply("failed");
  double anchor_x{}, anchor_y{};
  if (!Number(args, "anchorX", &anchor_x) || !Number(args, "anchorY", &anchor_y) ||
      anchor_x != projection_.caret_x || anchor_y != projection_.caret_y) return Reply("failed");
  const auto* glyphs = Array(args, "glyphs"); const auto* carets = Array(args, "carets");
  if (!glyphs || !carets || glyphs->size() > ni::kTextUnits * 2 || carets->size() > ni::kTextUnits + 1) return Reply("failed");
  std::array<RECT, ni::kTextUnits + 1> rectangles{};
  std::array<bool, ni::kTextUnits + 1> has_glyph{}, has_caret{};
  for (const auto& entry : *carets) {
    const auto* caret = std::get_if<Map>(&entry); int64_t offset{}; double x{}, y{}, height{};
    if (!caret || !Integer(*caret, "offset", &offset) || offset < 0 || static_cast<size_t>(offset) > composition_.size() ||
        !Number(*caret, "x", &x) || !Number(*caret, "y", &y) || !Number(*caret, "height", &height) || height <= 0 ||
        std::abs(x) > 100000 || std::abs(y) > 100000 || height > 100000 || has_caret[static_cast<size_t>(offset)]) return Reply("failed");
    const size_t index = static_cast<size_t>(offset);
    if (index && index < composition_.size() && ni::HighSurrogate(composition_[index - 1]) && ni::LowSurrogate(composition_[index])) return Reply("failed");
    if (!geometry_.Representable(anchor_x + x, anchor_y + y, anchor_x + x + 1.0 / geometry_.dpr, anchor_y + y + height)) return Reply("failed");
    rectangles[index] = geometry_.Rect(host_.view, anchor_x + x, anchor_y + y, anchor_x + x + 1.0 / geometry_.dpr, anchor_y + y + height);
    has_caret[index] = true;
  }
  for (const auto& entry : *glyphs) {
    const auto* glyph = std::get_if<Map>(&entry); int64_t first{}, last{}; double l{}, t{}, r{}, b{};
    if (!glyph || !Integer(*glyph, "start", &first) || !Integer(*glyph, "end", &last) || first < 0 || last <= first ||
        static_cast<size_t>(last) > composition_.size() || !Number(*glyph, "left", &l) || !Number(*glyph, "top", &t) ||
        !Number(*glyph, "right", &r) || !Number(*glyph, "bottom", &b) || l > r || t > b ||
        std::abs(l) > 100000 || std::abs(r) > 100000 || std::abs(t) > 100000 || std::abs(b) > 100000) return Reply("failed");
    if (!geometry_.Representable(anchor_x + l, anchor_y + t, anchor_x + r, anchor_y + b)) return Reply("failed");
    const auto rectangle = geometry_.Rect(host_.view, anchor_x + l, anchor_y + t, anchor_x + r, anchor_y + b);
    for (int64_t i = first; i < last; ++i) {
      const size_t index = static_cast<size_t>(i);
      if (has_glyph[index]) UnionRect(&rectangles[index], &rectangles[index], &rectangle);
      else rectangles[index] = rectangle;
      has_glyph[index] = true;
    }
  }
  for (size_t i = 0; i <= composition_.size(); ++i) {
    if (!has_glyph[i] && !has_caret[i]) return Reply("failed");
  }
  const auto status = surface_->Store()->SetTextLayout(rectangles.data(), static_cast<ULONG>(composition_.size() + 1), revision);
  surface_->Pump();
  return Reply(SUCCEEDED(status) ? "laidOut" : "failed");
}
Map PaneRoot::Claim(uint64_t effect) {
  if (!effect || (claim_ && (claim_effect_ != effect || claim_delivered_))) return Reply("rejected");
  auto context = Context();
  if (!claim_) {
    if (!Active()) return Reply("rejected");
    auto request = Abi<GuosshNormalClaimRequest>(); request.effect = effect;
    auto claim = Abi<GuosshNormalClaimInfo>();
    const auto status = host_.api.claim(&context, &request, &claim);
    if (status == GUOSSH_NORMAL_BUSY) return Reply("pending");
    if (status != GUOSSH_NORMAL_OK) return Reply("rejected");
    claim_ = claim.claim; claim_effect_ = claim.effect; claim_action_ = claim.action;
    claim_rejection_ = claim.rejection; claim_payload_length_ = claim.payload_len;
    claim_token_ = Token();
  }
  const char* const actions[] = {"", "newTab", "splitRight", "splitDown", "closePane", "nextTab", "previousTab", "nextPane",
      "search", "settings", "toggleSidebar", "copy", "paste", "selectAll", "zoomIn", "zoomOut", "zoomReset"};
  auto abort = [&]() {
    auto ack = Abi<GuosshNormalAck>(); ack.claim = claim_; ack.kind = 2;
    const auto status = host_.api.ack(&context, &ack);
    if (status == GUOSSH_NORMAL_BUSY) return Reply("pending");
    if (status == GUOSSH_NORMAL_OK) { claim_ = 0; claim_token_.clear(); }
    else Fail("CLAIM_ABORT_PENDING");
    return Reply("rejected");
  };
  if (!Active() || !claim_ || claim_effect_ != effect || claim_rejection_ || claim_action_ == 0 || claim_action_ >= std::size(actions) ||
      claim_action_ == 13 || claim_token_.empty() || claim_payload_length_ > kPayloadLimit) return abort();
  auto copy = Abi<GuosshNormalPayloadCopy>(); copy.claim = claim_;
  uint64_t length{};
  auto status = host_.api.action_payload(&context, &copy, &length);
  if (status == GUOSSH_NORMAL_BUSY) return Reply("pending");
  if (status != GUOSSH_NORMAL_OK || length != claim_payload_length_ || length > kPayloadLimit) return abort();
  std::string payload(static_cast<size_t>(length), '\0');
  copy.buffer = length ? reinterpret_cast<uint8_t*>(payload.data()) : nullptr; copy.capacity = length;
  uint64_t actual{};
  status = host_.api.action_payload(&context, &copy, &actual);
  if (status == GUOSSH_NORMAL_BUSY) return Reply("pending");
  if (status != GUOSSH_NORMAL_OK || actual != length) return abort();
  auto result = Reply("claimed");
  result[Value("claimToken")] = Value(claim_token_); result[Value("action")] = Value(actions[claim_action_]);
  result[Value("payload")] = Value(std::move(payload));
  claim_delivered_ = true;
  return result;
}
Map PaneRoot::Complete(const Map& args) {
  const auto* token = String(args, "claimToken"); const auto* kind = String(args, "kind");
  if (!claim_ || !token || *token != claim_token_ || !kind) return Reply("failed");
  auto ack = Abi<GuosshNormalAck>(); ack.claim = claim_;
  if (*kind == "completed") { if (claim_action_ == 12) return Reply("failed"); ack.kind = 1; }
  else if (*kind == "abort") ack.kind = 2;
  else if (*kind == "paste") {
    const auto* text = String(args, "text");
    if (claim_action_ != 12 || !text || text->size() > kPayloadLimit) return Reply("failed");
    if (!Active() || GetFocus() != hwnd_) return Reply("failed");
    ack.kind = 3; ack.text = reinterpret_cast<const uint8_t*>(text->data()); ack.text_len = text->size();
  } else return Reply("failed");
  auto context = Context();
  const auto status = host_.api.ack(&context, &ack);
  if (status == GUOSSH_NORMAL_BUSY) return Reply("pending");
  if (status != GUOSSH_NORMAL_OK) return Reply("failed");
  claim_ = claim_effect_ = 0; claim_action_ = 0; claim_delivered_ = false; claim_token_.clear();
  auto result = Reply(ack.kind == 2 ? "aborted" : "completed");
  Poll();
  return result;
}
}  // namespace guosshell::desktop_native_input
