#include "host_internal.h"
#include "channel.h"
#include <flutter/standard_method_codec.h>
#include <cstdio>
#include <cstring>
#include <vector>

using namespace guosshell::desktop_native_input;
namespace guosshell::desktop_native_input {
struct HostTestAccess {
  static ni::NativeSurface* Surface(PaneRoot& root) { return root.surface_.Get(); }
  static TfClientId Client(PaneRoot& root) { return root.host_.client; }
  static bool PrepareEmission(PaneRoot& root, HWND window, const ni::OwnerRef& owner) {
    root.surface_.Attach(ni::NativeSurface::Create(window, owner, nullptr,
        Permit::Acquire(root.host_.budget), ni::NativeOrigin::VerifiedNative));
    root.composition_ = L"x"; root.composition_ticket_ = 71; root.composition_revision_ = 3;
    root.projection_.frame_revision = 7;
    return root.surface_ != nullptr;
  }
  static void Emit(PaneRoot& root, bool ended) { root.EmitComposition(ended); }
};
struct ChannelTestAccess {
  static void Attach(Channel& channel, std::unique_ptr<Host> host) {
    channel.host_ = std::move(host);
    channel.host_->SetPollCallback([&channel] { channel.PollPending(); });
  }
};
}
namespace {
int checks{}, failures{};
void Check(bool okay, const char* name) { ++checks; if (!okay) { ++failures; std::printf("FAIL %s\n", name); } }
struct Entry {
  HWND window{}; uint64_t attempt{}, epoch{}, context{}, ticket{}, native_id{}, claim{}, end{};
  bool revoked{}, committed{}, released{};
  uint64_t reserved{}; std::string payload;
  uint64_t end_transaction{13};
};
std::map<uint64_t, Entry> entries;
std::map<uint64_t, uint64_t> aliases;
uint64_t next_handle = 100, epoch = 1;
uint32_t action = 11;
bool pointer_busy{};
bool reserve_busy{};
bool observe_key_only{};
GuosshNormalKey observed_key{};
int early_cancels{};
int claim_busy{}, payload_busy{}, ack_busy{}, end_ack_busy{}, claims{};
class Messenger final : public flutter::BinaryMessenger {
 public:
  void Send(const std::string&, const uint8_t*, size_t, flutter::BinaryReply) const override {}
  void SetMessageHandler(const std::string&, flutter::BinaryMessageHandler value) override { handler = std::move(value); }
  flutter::BinaryMessageHandler handler;
};
int registered{}, keys{}, reserves{}, finishes{}, revokes{}, releases{}, acks{}, pointers{}, presentations{}, end_acks{};
std::string copy_payload(65536, 'x');
std::string expected_payload = " ";
class Edit final : public ITfEditSession {
 public:
  explicit Edit(std::function<HRESULT(TfEditCookie)> operation) : operation_(std::move(operation)) {}
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITfEditSession) return E_NOINTERFACE;
    *out = static_cast<ITfEditSession*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return ++references_; }
  ULONG STDMETHODCALLTYPE Release() override { const ULONG left = --references_; if (!left) delete this; return left; }
  HRESULT STDMETHODCALLTYPE DoEditSession(TfEditCookie cookie) override { return operation_(cookie); }
 private:
  ULONG references_{1};
  std::function<HRESULT(TfEditCookie)> operation_;
};
class CompositionSink final : public ITfCompositionSink {
 public:
  int terminations{};
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITfCompositionSink) return E_NOINTERFACE;
    *out = static_cast<ITfCompositionSink*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return ++references_; }
  ULONG STDMETHODCALLTYPE Release() override { const ULONG left = --references_; if (!left) delete this; return left; }
  HRESULT STDMETHODCALLTYPE OnCompositionTerminated(TfEditCookie, ITfComposition*) override { ++terminations; return S_OK; }
 private: ULONG references_{1};
};
void RunEdit(PaneRoot& root, DWORD flags, std::function<HRESULT(TfEditCookie)> operation) {
  ComPtr<Edit> edit; edit.Attach(new Edit(std::move(operation)));
  HRESULT inner = E_FAIL;
  const HRESULT outer = HostTestAccess::Surface(root)->Context()->RequestEditSession(HostTestAccess::Client(root), edit.Get(), flags | TF_ES_SYNC, &inner);
  Check(SUCCEEDED(outer) && SUCCEEDED(inner), "real-normal-acp-edit-session");
}
GuosshNormalProjection projection = [] {
  auto value = Abi<GuosshNormalProjection>(); value.frame_revision = 7; value.row_revision = 9;
  value.viewport_top = value.screen_top_stable_row = 100; value.caret_present = 1;
  value.caret_x = 20; value.caret_y = 40; value.caret_width = 10; value.caret_height = 20; value.dpi = 96;
  return value;
}();
Entry& Original(const GuosshNormalContext* context) {
  Check(context && context->header.size == sizeof(*context) && context->session_id == 37 && context->generation == 9 &&
      context->identity_len == 8 && std::memcmp(context->identity, "identity", 8) == 0, "fixed-context-memory");
  const auto alias = aliases.find(context->handle);
  auto& entry = entries.at(alias == aliases.end() ? context->handle : alias->second);
  Check(entry.attempt == context->attempt && entry.epoch == context->focus_epoch, "fixed-binding");
  return entry;
}
NormalApi Api() {
  NormalApi api{};
  api.register_trusted_host = [](const GuosshNormalRegistration* request, GuosshNormalBound* out) -> uint32_t {
    HWND window = reinterpret_cast<HWND>(request->window);
    Check(IsWindow(window) && GetFocus() == window && GetPropW(window, GUOSSH_NORMAL_WINDOWS_INPUT_PROPERTY) ==
        reinterpret_cast<HANDLE>(request->input_object), "real-window-property-registration");
    ++registered; const uint64_t handle = ++next_handle;
    entries[handle] = {window, request->attempt, ++epoch};
    *out = Abi<GuosshNormalBound>(); out->handle = handle; out->attempt = request->attempt; out->focus_epoch = epoch;
    return GUOSSH_NORMAL_OK;
  };
  api.bind = [](const GuosshNormalContext* context, GuosshNormalBound* out) -> uint32_t {
    auto& entry = Original(context);
    Check(!entry.revoked && aliases.find(context->handle) == aliases.end(), "bind-original-registration");
    const uint64_t handle = ++next_handle; aliases[handle] = context->handle;
    *out = Abi<GuosshNormalBound>(); out->handle = handle; out->attempt = entry.attempt; out->focus_epoch = entry.epoch;
    return 0;
  };
  api.key = [](const GuosshNormalContext* context, const GuosshNormalKey* key, GuosshNormalAdmission* out) -> uint32_t {
    auto& entry = Original(context); ++keys;
    observed_key = *key;
    if (observe_key_only) return GUOSSH_NORMAL_POLICY_REJECTED;
    if (!key->raw_known || !key->same_event_dual || ((key->modifiers & 4) && !key->alt_provenance)) return GUOSSH_NORMAL_POLICY_REJECTED;
    Check(!entry.revoked && GetFocus() == entry.window && key->raw_known && key->same_event_dual, "key-native-admission");
    *out = Abi<GuosshNormalAdmission>();
    if (key->phase == 3) { out->route = 3; return 0; }
    entry.context = key->context_id;
    if (!entry.ticket || entry.committed) { entry.ticket = 70 + keys; entry.committed = false; out->text_op = 1; }
    else { Check(key->resume_text_ticket == entry.ticket, "same-ticket-continue"); out->text_op = 2; }
    out->effect = out->text_ticket = entry.ticket; out->route = 2; out->cycle_id = 1;
    return 0;
  };
  api.reserve_payload = [](const GuosshNormalContext* context, const GuosshNormalTextReservation* request) -> uint32_t {
    auto& entry = Original(context); ++reserves;
    Check(request->text_ticket == entry.ticket && request->utf8_bytes == expected_payload.size(), "exact-reservation-not-capacity");
    if (reserve_busy) return GUOSSH_NORMAL_BUSY;
    entry.reserved = request->utf8_bytes; entry.payload.clear(); return 0;
  };
  api.payload = [](const GuosshNormalContext* context, const GuosshNormalTextChunk* request) -> uint32_t {
    auto& entry = Original(context);
    Check(request->offset == entry.payload.size() && request->text_ticket == entry.ticket, "payload-contiguous-original-ticket");
    entry.payload.append(reinterpret_cast<const char*>(request->bytes), static_cast<size_t>(request->len)); return 0;
  };
  api.finish = [](const GuosshNormalContext* context, const GuosshNormalTextFinish* request) -> uint32_t {
    auto& entry = Original(context); ++finishes;
    Check(entry.payload.size() == entry.reserved && entry.payload == expected_payload && request->text_ticket == entry.ticket, "finish-complete-text");
    entry.committed = true; return 0;
  };
  api.composition = [](const GuosshNormalContext* context, const GuosshNormalComposition* request) -> uint32_t {
    Original(context); if (request->kind == 3) ++early_cancels; return 0;
  };
  api.text_end_query = [](const GuosshNormalContext* context, GuosshNormalTextEnd* out) -> uint32_t {
    auto& entry = Original(context); *out = Abi<GuosshNormalTextEnd>();
    if (entry.end) { out->end_fence = entry.end; out->context_id = entry.context; out->transaction_id = entry.end_transaction; }
    return 0;
  };
  api.text_end_ack = [](const GuosshNormalContext* context, const GuosshNormalTextEnd* request) -> uint32_t {
    auto& entry = Original(context);
    if (end_ack_busy) { --end_ack_busy; return GUOSSH_NORMAL_BUSY; }
    ++end_acks;
    auto* root = static_cast<PaneRoot*>(GetPropW(entry.window, GUOSSH_NORMAL_WINDOWS_INPUT_PROPERTY));
    auto* surface = HostTestAccess::Surface(*root);
    Check(surface && !surface->Context() && surface->Store()->CanDetach() && request->end_fence == entry.end &&
        request->context_id == entry.context && request->transaction_id == entry.end_transaction, "end-ack-after-real-native-detach");
    entry.end = entry.ticket = 0; entry.end_transaction = 13; return 0;
  };
  api.projection = [](const GuosshNormalContext* context, GuosshNormalProjection* out) -> uint32_t { Original(context); *out = projection; return 0; };
  api.presented_geometry = [](const GuosshNormalContext* context, const GuosshNormalPresentedGeometry* request) -> uint32_t {
    auto& entry = Original(context); ++presentations;
    Check(request->native_request_id == entry.native_id + 1 && request->geometry.frame_revision == 7 &&
        request->geometry.row_revision == 9 && request->geometry.transform[4] == 10, "same-frame-native-counter");
    entry.native_id = request->native_request_id; return 0;
  };
  api.invalidate_geometry = [](const GuosshNormalContext* context, uint64_t request) -> uint32_t {
    auto& entry = Original(context); Check(request == entry.native_id + 1, "invalidate-native-counter"); entry.native_id = request; return 0;
  };
  api.pointer = [](const GuosshNormalContext* context, const GuosshNormalPointer* request, GuosshNormalAdmission* out) -> uint32_t {
    auto& entry = Original(context); ++pointers;
    Check(request->native_request_id == entry.native_id + 1 && request->input.stable_hit_row == 101 &&
        request->input.column == 2 && request->input.remote_row == 1, "pointer-frozen-hit");
    if (pointer_busy) return GUOSSH_NORMAL_BUSY;
    entry.native_id = request->native_request_id; *out = Abi<GuosshNormalAdmission>(); out->route = 1; return 0;
  };
  api.claim = [](const GuosshNormalContext* context, const GuosshNormalClaimRequest* request, GuosshNormalClaimInfo* out) -> uint32_t {
    auto& entry = Original(context);
    if (claim_busy) { --claim_busy; return GUOSSH_NORMAL_BUSY; }
    ++claims; entry.claim = 900 + request->effect;
    *out = Abi<GuosshNormalClaimInfo>(); out->claim = entry.claim; out->effect = request->effect; out->action = action;
    out->payload_len = action == 11 ? copy_payload.size() : 0; return 0;
  };
  api.action_payload = [](const GuosshNormalContext* context, const GuosshNormalPayloadCopy* request, uint64_t* count) -> uint32_t {
    auto& entry = Original(context); Check(request->claim == entry.claim, "copy-original-claim");
    if (payload_busy) { --payload_busy; return GUOSSH_NORMAL_BUSY; }
    const auto length = action == 11 ? copy_payload.size() : 0;
    if (request->buffer) { Check(request->capacity >= length, "copy-full-capacity"); std::memcpy(request->buffer, copy_payload.data(), length); }
    *count = length; return 0;
  };
  api.ack = [](const GuosshNormalContext* context, const GuosshNormalAck* request) -> uint32_t {
    auto& entry = Original(context);
    if (ack_busy) { --ack_busy; return GUOSSH_NORMAL_BUSY; }
    ++acks;
    Check(request->claim == entry.claim, "ack-original-claim");
    if (request->kind == 3 && entry.revoked) return GUOSSH_NORMAL_STALE_FOCUS;
    entry.claim = 0; return 0;
  };
  api.revoke = [](const GuosshNormalContext* context) -> uint32_t {
    auto& entry = Original(context); ++revokes; entry.revoked = true;
    if (entry.ticket && !entry.committed) entry.end = 43;
    return 0;
  };
  api.release_retired = [](const GuosshNormalContext* context) -> uint32_t {
    auto& entry = Original(context);
    Check(entry.revoked && !entry.claim && !entry.end, "release-after-quiet-and-claim");
    if (aliases.erase(context->handle)) return 0;
    ++releases; entry.released = true; return 0;
  };
  return api;
}
Map GeometryArgs() {
  return {{Value("x"), Value(10.0)}, {Value("y"), Value(20.0)}, {Value("width"), Value(200.0)}, {Value("height"), Value(100.0)},
    {Value("clip"), Value(Map{{Value("left"), Value(10.0)}, {Value("top"), Value(20.0)}, {Value("right"), Value(210.0)}, {Value("bottom"), Value(120.0)}})},
    {Value("cellWidth"), Value(10.0)}, {Value("cellHeight"), Value(20.0)}, {Value("scrollOffset"), Value(0.0)}, {Value("devicePixelRatio"), Value(1.0)},
    {Value("fontRevision"), Value("1")}, {Value("layoutRevision"), Value("2")}, {Value("transformRevision"), Value("3")}, {Value("viewportRevision"), Value("4")},
    {Value("transform"), Value(List{Value(1.0),Value(0.0),Value(0.0),Value(0.0),Value(0.0),Value(1.0),Value(0.0),Value(0.0),
      Value(0.0),Value(0.0),Value(1.0),Value(0.0),Value(10.0),Value(20.0),Value(0.0),Value(1.0)})}};
}
class WireReply final : public flutter::MethodResult<Value> {
 public:
  Map value;
  bool success{}, finished{};
 protected:
  void SuccessInternal(const Value* result) override {
    finished = true;
    if (result) if (const auto* map = std::get_if<Map>(result)) { value = *map; success = true; }
  }
  void ErrorInternal(const std::string&, const std::string&, const Value*) override { finished = true; }
  void NotImplementedInternal() override { finished = true; }
};
std::shared_ptr<WireReply> BeginWire(Messenger& messenger, const char* method, Map args) {
  args[Value("version")] = Value(int32_t{1});
  auto reply = std::make_shared<WireReply>();
  const auto& codec = flutter::StandardMethodCodec::GetInstance();
  auto bytes = codec.EncodeMethodCall(flutter::MethodCall<Value>(method, std::make_unique<Value>(std::move(args))));
  messenger.handler(bytes->data(), bytes->size(), [reply](const uint8_t* data, size_t size) {
    Check(flutter::StandardMethodCodec::GetInstance().DecodeAndProcessResponseEnvelope(data, size, reply.get()), "deferred-method-envelope");
  });
  return reply;
}
void WaitWire(Host& host, const std::shared_ptr<WireReply>& reply) {
  const auto deadline = GetTickCount64() + 4500;
  while (!reply->finished && GetTickCount64() < deadline) {
    MSG message{};
    while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
      if (!host.BeforeTranslate(message)) { TranslateMessage(&message); DispatchMessageW(&message); }
    }
    host.Pump();
    if (!reply->finished) MsgWaitForMultipleObjects(0, nullptr, FALSE, 16, QS_ALLINPUT);
  }
  Check(reply->finished && reply->success, "deferred-original-method-completes");
}
Map Call(Host& host, const char* method, Map args) {
  args[Value("version")] = Value(int32_t{1});
  const auto& codec = flutter::StandardMethodCodec::GetInstance();
  auto wire = codec.EncodeMethodCall(flutter::MethodCall<Value>(method, std::make_unique<Value>(args)));
  auto decoded = codec.DecodeMethodCall(wire->data(), wire->size());
  Check(decoded && decoded->method_name() == method, "normal-standard-codec");
  const Value result(host.Call(method, std::get<Map>(*decoded->arguments())));
  const auto envelope = codec.EncodeSuccessEnvelope(&result);
  WireReply reply;
  Check(codec.DecodeAndProcessResponseEnvelope(envelope->data(), envelope->size(), &reply) && reply.success, "normal-response-codec");
  return reply.value;
}
bool Is(const Map& result, const char* state) { const auto* actual = String(result, "state"); return actual && *actual == state; }
Map Pane(const std::string& token) { return {{Value("paneToken"), Value(token)}}; }
std::string Attach(Host& host) {
  auto result = Call(host, "attachPane", {{Value("source"), Value(Map{{Value("sessionId"), Value(int32_t{37})}, {Value("generation"), Value("9")}})},
      {Value("identity"), Value("identity")}, {Value("attempt"), Value("1")}, {Value("focusEpoch"), Value("1")},
      {Value("lifecycleRevision"), Value("1")}, {Value("geometry"), Value(GeometryArgs())}});
  Check(Is(result, "attached"), "pane-attached");
  return String(result, "paneToken") ? *String(result, "paneToken") : std::string{};
}
PaneRoot* Root(HWND parent, const std::string& token) {
  struct FindRoot { const std::string& token; PaneRoot* found{}; } state{token};
  EnumChildWindows(parent, [](HWND window, LPARAM data) -> BOOL {
    auto* state = reinterpret_cast<FindRoot*>(data);
    auto* root = static_cast<PaneRoot*>(GetPropW(window, GUOSSH_NORMAL_WINDOWS_INPUT_PROPERTY));
    if (root && root->token_ == state->token) state->found = root;
    return TRUE;
  }, reinterpret_cast<LPARAM>(&state));
  return state.found;
}
void RunCompositionEmission(HWND window) {
  // 只验证通知发送边界；不激活输入域，也不替代真实 TSF 终结实证。
  const int before_checks = checks, before_failures = failures;
  std::vector<Map> notifications;
  NativeHostState host;
  host.notify = [&](const char* method, Map value) {
    Check(std::strcmp(method, "compositionLayout") == 0, "composition-notification-channel");
    notifications.push_back(std::move(value));
  };
  PaneRoot root(host, 5, "original-pane", 37, 9, "identity", 1, 1);
  const ni::OwnerRef owner{5, 5, 37, 9, 5, 11};
  Check(HostTestAccess::PrepareEmission(root, window, owner), "composition-emission-owner");
  root.geometry_.valid = true;
  HostTestAccess::Emit(root, false);
  Check(notifications.size() == 1 && Is(notifications.back(), "open"), "valid-geometry-publishes-open");
  root.geometry_.valid = false;
  HostTestAccess::Emit(root, false);
  Check(notifications.size() == 1, "invalid-geometry-does-not-publish-open");
  HostTestAccess::Emit(root, true);
  Check(notifications.size() == 2 && Is(notifications.back(), "ended"), "invalid-geometry-still-publishes-ended");
  if (notifications.size() == 2) {
    const auto& ended = notifications.back();
    Check(String(ended, "text") && String(ended, "text")->empty(), "ended-clears-preedit-text");
    for (const char* field : {"paneToken", "contextId", "transactionId", "editRevision", "frameRevision", "acpStart"}) {
      const auto* original = Find(notifications.front(), field);
      const auto* actual = Find(ended, field);
      Check(original && actual && *original == *actual, "ended-keeps-original-composition-identity");
    }
    uint64_t context{}, transaction{};
    Check(U64(ended, "contextId", &context) && context == owner.context &&
        U64(ended, "transactionId", &transaction) && transaction == 71, "ended-targets-original-owner-ticket");
  }
  HostTestAccess::Emit(root, true);
  Check(notifications.size() == 2, "original-composition-ended-not-republished");
  std::printf("composition-emission checks=%d failures=%d; geometry_valid=%d notifications=%zu\n",
      checks - before_checks, failures - before_failures, root.geometry_.valid, notifications.size());
}
void Run(HWND parent) {
  // 独立 editable 表示终端外域；测试只核验命中，不取正文、不把焦点交给它。
  HWND other_field = CreateWindowExW(0, L"EDIT", L"", WS_CHILD | WS_VISIBLE, 0, 0, 100, 15,
      parent, nullptr, GetModuleHandleW(nullptr), nullptr);
  Check(other_field != nullptr, "independent-editable-domain");
  std::vector<Map> notifications;
  std::function<void(const Map&)> observe_ended;
  Messenger messenger;
  Channel channel(&messenger, parent);
  auto core = std::make_unique<Host>(parent, Api(), [&](const char*, Map value) {
    if (observe_ended && Is(value, "ended")) observe_ended(value);
    notifications.push_back(std::move(value));
  });
  Host& host = *core;
  ChannelTestAccess::Attach(channel, std::move(core));
  const auto token = Attach(host);
  auto focus = Pane(token); Put(focus, "requestId", 1);
  Check(Is(Call(host, "requestFocus", focus), "active"), "real-pane-focus");
  auto* root = Root(parent, token);
  Check(root && root->Active(), "opaque-token-real-owner");
  if (root && !root->Active()) std::printf("focus failure=%s own_foreground=%d\n", root->failure_.c_str(), GetForegroundWindow() == parent);
  if (!root || !root->Active()) return;
  RECT unpainted{}; TsViewCookie view_cookie{};
  auto* initial_store = HostTestAccess::Surface(*root)->Store();
  initial_store->GetActiveView(&view_cookie);
  Check(initial_store->GetScreenExt(view_cookie, &unpainted) == TS_E_NOLAYOUT, "unpainted-pane-does-not-invent-grid-layout");
  auto frame = GeometryArgs(); frame[Value("paneToken")] = Value(token);
  frame[Value("frameRevision")] = Value(int32_t{7}); Put(frame, "rowRevision", 9); Put(frame, "attempt", 1); Put(frame, "focusEpoch", root->bound_.focus_epoch);
  frame[Value("viewportTop")] = Value(int64_t{100});
  frame[Value("caret")] = Value(Map{{Value("x"),Value(20.0)},{Value("y"),Value(40.0)},{Value("width"),Value(10.0)},
      {Value("height"),Value(20.0)},{Value("dpi"),Value(int32_t{96})}});
  auto invalid = frame; auto matrix = *Array(invalid, "transform"); matrix[3] = Value(.01); invalid[Value("transform")] = Value(matrix);
  Check(Is(Call(host, "presentedFrame", invalid), "failed") && presentations == 0, "perspective-not-reinterpreted");
  invalid = frame; Put(invalid, "rowRevision", 8);
  Check(Is(Call(host, "presentedFrame", invalid), "failed") && presentations == 0, "wrong-render-frame-rejected");
  Check(Is(Call(host, "presentedFrame", frame), "presented"), "same-render-presented");
  POINT inside{30,40}, outside{5,5}; ClientToScreen(parent, &inside); ClientToScreen(parent, &outside);
  Check(WindowFromPoint(inside) == root->hwnd_ && WindowFromPoint(outside) == other_field, "native-input-layer-clips-hit-domain");
  auto* surface = HostTestAccess::Surface(*root);
  RawPressState press; std::array<BYTE,256> initial{}; press.Reset(initial, surface->Profile().hkl);
  RAWKEYBOARD packet{}; packet.MakeCode = 0x39; packet.VKey = VK_SPACE; packet.Message = WM_KEYDOWN;
  ni::NativeKey key{};
  Check(press.Decode(root->hwnd_, packet, 1, surface->Profile(), false, &key) == RawStatus::Ready, "constructed-canonical-packet");
  key.source_epoch = root->raw_epoch_;
  const auto dispatched = surface->DispatchCanonical(key, nullptr);
  Check(dispatched.translates == 1 && finishes == 1 && reserves == 1, "normal-gate-reserve-payload-finish");
  host.Pump();
  Check(root->Active() && revokes == 0 && root->next_context_ == 2, "normal-context-rotation-not-revoke");
  surface = HostTestAccess::Surface(*root);
  Check(press.Decode(root->hwnd_, packet, 2, surface->Profile(), false, &key) == RawStatus::Ready, "composition-canonical-repeat");
  key.source_epoch = root->raw_epoch_;
  // 这里只补构造 SPACE 包的命名键身份；不把测试包当作物理来源证明。
  key.raw = key.legacy = {ni::MetadataKind::Named, VK_SPACE}; key.trusted_raw = true;
  observe_key_only = true;
  auto modifier_sample = key;
  modifier_sample.physical_state[VK_MENU] = modifier_sample.physical_state[VK_RMENU] = 0x80;
  root->Gate(surface->Owner(), modifier_sample, nullptr);
  Check(observed_key.text_ownership == 1 && !(observed_key.modifiers & 2), "ordinary-right-alt-is-not-invented-altgr");
  modifier_sample.queue_state[VK_CONTROL] = 0x80;
  root->Gate(surface->Owner(), modifier_sample, nullptr);
  Check(observed_key.text_ownership == 2 && !(observed_key.modifiers & 2), "tsf-compatible-control-does-not-pollute-physical");
  modifier_sample.physical_state[VK_CONTROL] = modifier_sample.physical_state[VK_LCONTROL] = 0x80;
  root->Gate(surface->Owner(), modifier_sample, nullptr);
  Check(observed_key.alt_provenance == 0, "ambiguous-raw-control-right-alt-is-not-known");
  observe_key_only = false;
  const auto receipt = root->Gate(surface->Owner(), key, nullptr);
  Check(receipt.disposition == ni::Disposition::AllowNativeText && SUCCEEDED(surface->Store()->Bind(receipt.ticket)), "composition-original-ticket");
  Check(SUCCEEDED(surface->Store()->BeginDispatch({receipt.ticket, surface->Owner().context, 2, ni::TextRoute::TsfStore})), "composition-authorized-edit");
  ComPtr<CompositionSink> sink; sink.Attach(new CompositionSink());
  ComPtr<ITfComposition> composition;
  RunEdit(*root, TF_ES_READWRITE, [&](TfEditCookie cookie) {
    ComPtr<ITfRange> range;
    auto hr = surface->Context()->GetStart(cookie, &range);
    ComPtr<ITfContextComposition> owner;
    if (SUCCEEDED(hr)) hr = surface->Context()->QueryInterface(IID_PPV_ARGS(&owner));
    if (SUCCEEDED(hr)) hr = owner->StartComposition(cookie, range.Get(), sink.Get(), &composition);
    if (SUCCEEDED(hr)) hr = range->SetText(cookie, 0, L"\xd83d\xde42x", 3);
    return hr;
  });
  Check(SUCCEEDED(surface->Store()->EndDispatch()), "composition-real-onendedit-preedit");
  surface->Pump();
  Map composition_message;
  for (const auto& note : notifications) if (Is(note, "open")) composition_message = note;
  Check(String(composition_message, "text") && *String(composition_message, "text") == "\xf0\x9f\x99\x82x", "normal-preedit-full-utf8-notification");
  Check(press.Decode(root->hwnd_, packet, 3, surface->Profile(), false, &key) == RawStatus::Ready, "continue-repeat-packet");
  key.source_epoch = root->raw_epoch_;
  const auto continuation = surface->DispatchCanonical(key, nullptr);
  Check(continuation.disposition == ni::Disposition::AllowNativeText && !continuation.translates && finishes == 1 &&
      *surface->Store()->OriginalTicket() == receipt.ticket, "continue-keeps-original-transaction");
  auto glyph_layout = GeometryArgs();
  for (const char* field : {"paneToken", "contextId", "transactionId", "editRevision", "frameRevision", "acpStart"})
    if (const auto* value = Find(composition_message, field)) glyph_layout[Value(field)] = *value;
  glyph_layout[Value("anchorX")] = Value(20.0); glyph_layout[Value("anchorY")] = Value(40.0);
  glyph_layout[Value("width")] = Value(30.0); glyph_layout[Value("height")] = Value(20.0);
  glyph_layout[Value("glyphs")] = Value(List{
    Value(Map{{Value("start"),Value(int32_t{0})},{Value("end"),Value(int32_t{2})},{Value("left"),Value(0.0)},
      {Value("top"),Value(0.0)},{Value("right"),Value(20.0)},{Value("bottom"),Value(20.0)}}),
    Value(Map{{Value("start"),Value(int32_t{2})},{Value("end"),Value(int32_t{3})},{Value("left"),Value(20.0)},
      {Value("top"),Value(0.0)},{Value("right"),Value(30.0)},{Value("bottom"),Value(20.0)}})});
  List carets;
  for (int32_t offset : {0, 2, 3}) carets.emplace_back(Map{{Value("offset"),Value(offset)}, {Value("x"),Value(offset * 10.0)},
    {Value("y"),Value(0.0)}, {Value("height"),Value(20.0)}});
  glyph_layout[Value("carets")] = Value(carets);
  Check(Is(Call(host, "textLayout", glyph_layout), "laidOut"), "normal-glyph-layout-accepted");
  RunEdit(*root, TF_ES_READ, [&](TfEditCookie) {
    RECT actual{}; BOOL clipped{};
    TsViewCookie active_view{};
    auto hr = surface->Store()->GetActiveView(&active_view);
    if (SUCCEEDED(hr)) hr = surface->Store()->GetTextExt(active_view, 0, 2, &actual, &clipped);
    const auto expected = root->geometry_.Rect(parent, 20, 40, 40, 60);
    Check(SUCCEEDED(hr) && EqualRect(&actual, &expected), "real-acp-range-uses-glyph-screen-rect");
    return hr;
  });
  expected_payload = "\xf0\x9f\x99\x82x";
  const size_t before_end = notifications.size();
  Check(root->Invalidate() && !root->geometry_.valid, "composition-end-after-geometry-invalidation");
  Check(Is(Call(host, "textLayout", glyph_layout), "failed"), "invalid-geometry-still-rejects-text-layout");
  RunEdit(*root, TF_ES_READWRITE, [&](TfEditCookie cookie) { return composition->EndComposition(cookie); });
  composition.Reset(); sink.Reset(); surface->Pump();
  Check(finishes == 2 && reserves == 2, "real-composition-end-exact-payload-once");
  Check(surface->Store()->Sealed() && surface->Store()->CanDetach(), "real-composition-end-is-native-quiet-before-rotation");
  Check(notifications.size() == before_end + 1 && Is(notifications.back(), "ended"), "real-composition-ended-with-invalid-geometry");
  if (notifications.size() == before_end + 1) {
    const auto& ended = notifications.back();
    for (const char* field : {"paneToken", "contextId", "transactionId", "editRevision"}) {
      const auto* original = Find(composition_message, field);
      const auto* actual = Find(ended, field);
      Check(original && actual && *original == *actual, "real-end-keeps-original-owner-identity");
    }
    Check(String(ended, "text") && String(ended, "text")->empty(), "real-end-clears-original-preedit");
  }
  host.Pump();
  Check(root->Active() && revokes == 0 && root->next_context_ == 3, "composition-context-rotation-keeps-press");
  Check(notifications.size() == before_end + 1, "normal-context-rotation-does-not-double-ended");
  expected_payload = " ";
  action = 11; auto claim_args = Pane(token); Put(claim_args, "effect", 201);
  claim_busy = payload_busy = 1;
  const auto claiming = BeginWire(messenger, "claimAction", claim_args);
  Check(!claiming->finished && !root->claim_, "claim-busy-does-not-reject-or-block-ui");
  WaitWire(host, claiming);
  auto claim = claiming->value;
  Check(claims == 1, "payload-busy-reuses-original-claim");
  Check(Is(claim, "claimed") && String(claim, "payload") && *String(claim, "payload") == copy_payload, "complete-copy-payload");
  const std::string claim_token = String(claim, "claimToken") ? *String(claim, "claimToken") : "";
  auto start_preedit = [&](uint64_t event, const WCHAR* text, ULONG count) {
    surface = HostTestAccess::Surface(*root);
    Check(press.Decode(root->hwnd_, packet, event, surface->Profile(), false, &key) == RawStatus::Ready,
        "forced-end-canonical-test-packet");
    key.source_epoch = root->raw_epoch_;
    key.raw = key.legacy = {ni::MetadataKind::Named, VK_SPACE}; key.trusted_raw = true;
    const auto admission = root->Gate(surface->Owner(), key, nullptr);
    Check(admission.disposition == ni::Disposition::AllowNativeText && SUCCEEDED(surface->Store()->Bind(admission.ticket)),
        "forced-end-original-ticket");
    Check(SUCCEEDED(surface->Store()->BeginDispatch({admission.ticket, surface->Owner().context, event, ni::TextRoute::TsfStore})),
        "forced-end-authorized-tsf-edit");
    sink.Attach(new CompositionSink());
    RunEdit(*root, TF_ES_READWRITE, [&](TfEditCookie cookie) {
      ComPtr<ITfRange> range;
      auto hr = surface->Context()->GetStart(cookie, &range);
      ComPtr<ITfContextComposition> owner;
      if (SUCCEEDED(hr)) hr = surface->Context()->QueryInterface(IID_PPV_ARGS(&owner));
      if (SUCCEEDED(hr)) hr = owner->StartComposition(cookie, range.Get(), sink.Get(), &composition);
      if (SUCCEEDED(hr)) hr = range->SetText(cookie, 0, text, count);
      return hr;
    });
    Check(SUCCEEDED(surface->Store()->EndDispatch()), "forced-end-real-onendedit-preedit");
    surface->Pump();
    Check(!notifications.empty() && Is(notifications.back(), "open"), "forced-end-visible-preedit");
    return notifications.back();
  };
  Check(Is(Call(host, "presentedFrame", frame), "presented"), "forced-end-presented-frame");
  const auto cancelled_preedit = start_preedit(4, L"cancel", 6);
  auto root_context = root->Context();
  auto& backend = Original(&root_context);
  const uint64_t cancelled_context = backend.context, cancelled_ticket = backend.ticket;
  // 非键 GUI 意图要求封口；C ABI 替身返回原事务 fence，原生退休仍由真实 TSF 完成。
  backend.end = 42; backend.end_transaction = cancelled_ticket;
  auto end = Pane(token); Put(end, "contextId", cancelled_context); Put(end, "transactionId", cancelled_ticket); Put(end, "endFence", 42);
  const int before_cancel_finishes = finishes, before_cancel_reserves = reserves;
  const size_t before_cancel_notes = notifications.size();
  Check(root->Invalidate(), "forced-end-invalidates-geometry-without-root-revoke");
  auto held = ComPtr<ni::TextStore>(HostTestAccess::Surface(*root)->Store());
  composition.Reset();
  Check(Is(Call(host, "textEndRequired", end), "accepted") && end_acks == 0, "accepted-is-not-nativequiet");
  Check(sink->terminations == 1 && held->Retired() && !held->Composing() && root->Active() &&
      root->next_context_ == cancelled_context && notifications.size() == before_cancel_notes,
      "forced-end-awaits-original-store-without-clearing-overlay");
  Check(finishes == before_cancel_finishes && reserves == before_cancel_reserves, "forced-end-does-not-finish-cancelled-text");
  observe_ended = [&](const Map& note) {
    Check(surface->State() == ni::FocusState::Retired && !surface->Context() && end_acks == 1 && root->Active() &&
        root->next_context_ == cancelled_context && surface->Owner().context == cancelled_context,
        "forced-ended-after-original-quiet-ack-before-rotation");
    for (const char* field : {"paneToken", "contextId", "transactionId", "editRevision"}) {
      const auto* original = Find(cancelled_preedit, field);
      const auto* actual = Find(note, field);
      Check(original && actual && *original == *actual, "forced-ended-keeps-original-preedit-identity");
    }
    Check(String(note, "text") && String(note, "text")->empty(), "forced-ended-clears-original-overlay");
  };
  end_ack_busy = 1;
  held.Reset(); host.Pump();
  Check(end_acks == 0 && surface->State() == ni::FocusState::NativeRetiring && notifications.size() == before_cancel_notes,
      "busy-quiet-ack-is-not-overlay-clear");
  host.Pump();
  observe_ended = {};
  Check(end_acks == 1 && root->Active() && revokes == 0, "context-fence-ack-keeps-claim-root");
  Check(root->next_context_ == cancelled_context + 1 && notifications.size() == before_cancel_notes + 1 &&
      Is(notifications.back(), "ended") && finishes == before_cancel_finishes && reserves == before_cancel_reserves,
      "forced-context-retirement-publishes-ended-once-without-finish");
  std::printf("forced-ended root_active=%d quiet_acks=%d finish_delta=%d notification_delta=%zu original_context=%llu current_context=%llu\n",
      root->Active(), end_acks, finishes - before_cancel_finishes, notifications.size() - before_cancel_notes,
      static_cast<unsigned long long>(cancelled_context), static_cast<unsigned long long>(root->next_context_));
  sink.Reset();
  Check(Is(Call(host, "presentedFrame", frame), "presented"), "new-context-presented-after-forced-end");
  const auto newer_preedit = start_preedit(5, L"new", 3);
  const size_t before_late_notes = notifications.size();
  Check(Is(Call(host, "textEndRequired", end), "nativequiet") && end_acks == 1, "late-original-end-notification-is-idempotent");
  auto wrong_end = end; Put(wrong_end, "transactionId", cancelled_ticket + 1);
  Check(Is(Call(host, "textEndRequired", wrong_end), "failed") && end_acks == 1, "wrong-end-identity-not-acknowledged");
  Check(root->Active() && surface->State() == ni::FocusState::Active && surface->Store()->Composing() &&
      surface->Owner().context == cancelled_context + 1 && notifications.size() == before_late_notes &&
      finishes == before_cancel_finishes, "late-old-fence-does-not-end-new-context-preedit");
  expected_payload = "new";
  RunEdit(*root, TF_ES_READWRITE, [&](TfEditCookie cookie) { return composition->EndComposition(cookie); });
  composition.Reset(); sink.Reset(); surface->Pump(); host.Pump(); host.Pump();
  Check(root->Active() && root->next_context_ == cancelled_context + 2 && finishes == before_cancel_finishes + 1 &&
      notifications.size() == before_late_notes + 1 && Is(notifications.back(), "ended"), "new-context-finishes-and-ends-exactly-once");
  for (const char* field : {"paneToken", "contextId", "transactionId", "editRevision"}) {
    const auto* original = Find(newer_preedit, field);
    const auto* actual = Find(notifications.back(), field);
    Check(original && actual && *original == *actual, "new-ended-targets-new-context-only");
  }
  expected_payload = " ";
  auto retire = Pane(token); Put(retire, "requestId", 2); retire[Value("phase")] = Value("quiet");
  Check(Is(Call(host, "retirePane", retire), "nativequiet") && releases == 0 && acks == 0, "nativequiet-does-not-wait-for-gui-claim");
  Put(retire, "requestId", 3); retire[Value("phase")] = Value("release");
  Check(Is(Call(host, "retirePane", retire), "pending") && releases == 0, "release-waits-for-original-claim-only");
  auto complete = Pane(token); complete[Value("claimToken")] = Value(claim_token); complete[Value("kind")] = Value("completed");
  ack_busy = 1;
  const auto completing = BeginWire(messenger, "completeAction", complete);
  Check(!completing->finished && root->claim_ && !root->released_, "completion-busy-retains-original-claim");
  WaitWire(host, completing);
  Check(Is(completing->value, "completed") && acks == 1, "retired-original-copy-completed");
  host.Pump();
  bool release_notified{};
  for (const auto& note : notifications) {
    uint64_t request{};
    if (Is(note, "released") && U64(note, "requestId", &request) && request == 3) release_notified = true;
  }
  Check(release_notified && releases == 1, "released-exact-pending-request-after-ack");
  const auto paste_token = Attach(host); focus = Pane(paste_token); Put(focus, "requestId", 4);
  Check(Is(Call(host, "requestFocus", focus), "active"), "new-root-independent-focus");
  action = 12; claim_args = Pane(paste_token); Put(claim_args, "effect", 202); claim = Call(host, "claimAction", claim_args);
  const auto paste_claim = *String(claim, "claimToken");
  retire = Pane(paste_token); Put(retire, "requestId", 5); retire[Value("phase")] = Value("quiet");
  Check(Is(Call(host, "retirePane", retire), "nativequiet"), "paste-root-quiet");
  complete = Pane(paste_token); complete[Value("claimToken")] = Value(paste_claim); complete[Value("kind")] = Value("paste"); complete[Value("text")] = Value("late");
  Check(Is(Call(host, "completeAction", complete), "failed") && acks == 1, "late-paste-not-authorized");
  complete[Value("kind")] = Value("abort");
  Check(Is(Call(host, "completeAction", complete), "aborted") && acks == 2, "late-paste-original-abort");
  Put(retire, "requestId", 6); retire[Value("phase")] = Value("release");
  Check(Is(Call(host, "retirePane", retire), "released"), "paste-root-released");
  const auto pointer_token = Attach(host); focus = Pane(pointer_token); Put(focus, "requestId", 7);
  Check(Is(Call(host, "requestFocus", focus), "active"), "pointer-root-focus");
  auto* pointer_root = Root(parent, pointer_token);
  frame[Value("paneToken")] = Value(pointer_token); Put(frame, "focusEpoch", pointer_root->bound_.focus_epoch);
  Check(Is(Call(host, "presentedFrame", frame), "presented"), "pointer-presented-frame");
  MSG mouse{}; mouse.hwnd = pointer_root->hwnd_; mouse.message = WM_LBUTTONDOWN; mouse.wParam = MK_LBUTTON | MK_SHIFT; mouse.lParam = MAKELPARAM(25, 25);
  Check(host.BeforeTranslate(mouse) && pointers == 1, "normal-pointer-single-path");
  pointer_busy = true; mouse.message = WM_MOUSEMOVE;
  const int original_presentations = presentations;
  Check(host.BeforeTranslate(mouse) && pointers == 2 && !pointer_root->Active(), "busy-pointer-seals-source");
  Check(Is(Call(host, "presentedFrame", frame), "failed") && presentations == original_presentations,
      "geometry-cannot-pass-busy-pointer");
  retire = Pane(pointer_token); Put(retire, "requestId", 8); retire[Value("phase")] = Value("release");
  const auto retired_pointer = Call(host, "retirePane", retire);
  Check(Is(retired_pointer, "released"), "busy-pointer-root-retires");
  pointer_busy = false;
  const auto outside_token = Attach(host); focus = Pane(outside_token); Put(focus, "requestId", 14);
  Check(Is(Call(host, "requestFocus", focus), "active"), "captured-pointer-root-focus");
  auto* outside_root = Root(parent, outside_token);
  frame[Value("paneToken")] = Value(outside_token); Put(frame, "focusEpoch", outside_root->bound_.focus_epoch);
  Check(Is(Call(host, "presentedFrame", frame), "presented"), "captured-pointer-frame");
  mouse.hwnd = outside_root->hwnd_; mouse.message = WM_LBUTTONDOWN; mouse.lParam = MAKELPARAM(25, 25); mouse.wParam = MK_LBUTTON;
  Check(host.BeforeTranslate(mouse) && GetCapture() == outside_root->hwnd_, "pointer-press-captures-own-window");
  const int before_outside = pointers;
  mouse.message = WM_LBUTTONUP; mouse.lParam = MAKELPARAM(500, 500); mouse.wParam = 0;
  Check(host.BeforeTranslate(mouse) && GetCapture() != outside_root->hwnd_ && !outside_root->Active() && pointers == before_outside,
      "outside-release-restores-capture-without-fabricated-cell");
  retire = Pane(outside_token); Put(retire, "requestId", 15); retire[Value("phase")] = Value("release");
  Check(Is(Call(host, "retirePane", retire), "released"), "outside-pointer-root-retires");
  const auto retained_token = Attach(host); focus = Pane(retained_token); Put(focus, "requestId", 16);
  Check(Is(Call(host, "requestFocus", focus), "active"), "retained-com-root-focus");
  auto* retained_root = Root(parent, retained_token);
  ComPtr<ni::NativeSurface> retained_surface(HostTestAccess::Surface(*retained_root));
  retire = Pane(retained_token); Put(retire, "requestId", 17); retire[Value("phase")] = Value("quiet");
  Check(Is(Call(host, "retirePane", retire), "pending") && !retained_root->quiet_, "external-surface-keeps-original-root");
  retained_root->deadline_ = GetTickCount64() - 1;
  host.Pump();
  Check(retained_root->failed_ && !retained_root->quiet_ && !retained_root->released_, "deadline-is-failure-not-fake-quiet");
  retained_surface.Reset(); Put(retire, "requestId", 18); retire[Value("phase")] = Value("release");
  Check(Is(Call(host, "retirePane", retire), "released"), "explicit-retirement-collects-last-com-release");
  const auto cancelled_token = Attach(host); focus = Pane(cancelled_token); Put(focus, "requestId", 19);
  Check(Is(Call(host, "requestFocus", focus), "active"), "payload-failure-root-focus");
  auto* cancelled_root = Root(parent, cancelled_token);
  auto* cancelled_surface = HostTestAccess::Surface(*cancelled_root);
  ComPtr<ni::TextStore> cancel_holder(cancelled_surface->Store());
  press.Reset(initial, cancelled_surface->Profile().hkl);
  Check(press.Decode(cancelled_root->hwnd_, packet, 1, cancelled_surface->Profile(), false, &key) == RawStatus::Ready, "payload-failure-canonical-test-packet");
  key.source_epoch = cancelled_root->raw_epoch_;
  const int previous_acks = end_acks;
  reserve_busy = true;
  const auto cancelled = cancelled_surface->DispatchCanonical(key, nullptr);
  host.Pump();
  Check((FAILED(cancelled.native_result) || cancelled.fault != ni::Fault::None) && !cancelled_root->Active() &&
      early_cancels == 0 && end_acks == previous_acks && !cancelled_root->quiet_, "payload-failure-awaits-original-quiet-fence");
  cancel_holder.Reset(); reserve_busy = false; host.Pump();
  Check(end_acks == previous_acks + 1 && cancelled_root->quiet_, "payload-failure-cancel-only-after-nativequiet");
  retire = Pane(cancelled_token); Put(retire, "requestId", 20); retire[Value("phase")] = Value("release");
  Check(Is(Call(host, "retirePane", retire), "released"), "cancelled-payload-root-released");
  host.Close();
  const ULONGLONG deadline = GetTickCount64() + 5000;
  while (!host.Closed() && GetTickCount64() < deadline) { host.Pump(); MsgWaitForMultipleObjects(0, nullptr, FALSE, 10, QS_ALLINPUT); }
  Check(host.Closed(), "host-worker-stopped-without-ui-join");
  Check(aliases.empty(), "all-bound-callback-aliases-released");
  DestroyWindow(other_field);
}
}
int main() {
  Check(SUCCEEDED(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED)), "com-start");
  HWND window = CreateWindowExW(0, L"STATIC", L"normal host interface test", WS_OVERLAPPEDWINDOW, 0, 0, 320, 240, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
  Check(window != nullptr, "host-window");
  RunCompositionEmission(window);
  if (failures) { DestroyWindow(window); CoUninitialize(); return 1; }
  ShowWindow(window, SW_SHOW);
  const BOOL foreground = SetForegroundWindow(window);
  SetActiveWindow(window);
  std::printf("real-foreground SetForegroundWindow=%ld own_foreground=%d\n", foreground, GetForegroundWindow() == window);
  if (!foreground || GetForegroundWindow() != window) {
    std::printf("BLOCKED: Windows 拒绝真实前台域；不执行需要该权限的正常宿主联证\n");
    DestroyWindow(window); CoUninitialize(); return 2;
  }
  Run(window);
  SetFocus(nullptr); DestroyWindow(window); CoUninitialize();
  std::printf("normal-host checks=%d failures=%d; real HWND/COM/codec, C ABI test double, physical/TIP/Rinf unverified\n", checks, failures);
  return failures ? 1 : 0;
}
