#include "native_surface.h"

#include <cstdio>
#include <cstring>
#include <functional>
#include <string>

using namespace guossh::native_input;
using Microsoft::WRL::ComPtr;

namespace {
int failures{};
int checks{};
void Check(bool condition, unsigned label) {
  ++checks;
  if (!condition) { ++failures; std::printf("FAIL synthetic_%u\n", label); }
}
void Hr(HRESULT result, unsigned label) {
  ++checks;
  if (FAILED(result)) { ++failures; std::printf("FAIL synthetic_%u hr=%08lx\n", label, static_cast<unsigned long>(result)); }
}
struct Permit final : NativePermit {
  explicit Permit(int* live) : live(live) { ++*live; }
  ~Permit() override { --*live; }
  NativeMemory& Memory() noexcept override { static NativeMemory memory; return memory; }
  int* live;
};
struct Consumer final : Callbacks {
  GateReceipt Gate(const OwnerRef& owner, const NativeKey& key, const TicketRef* original) noexcept override {
    ++gates;
    TicketRef ticket = original ? *original : TicketRef{owner.generation, owner.context, key.event_id, key.event_id};
    return {disposition, ticket, key.trusted_raw, false};
  }
  bool ReservePayload(const TicketRef& ticket, uint32_t n, PayloadGrant* out) noexcept override {
    ++reserves; last = ticket;
    *out = {ticket.transaction, n, 7};
    if (on_reserve) on_reserve();
    return !fail_reserve;
  }
  bool Preedit(const TicketRef& ticket, uint64_t, const WCHAR*, ULONG, LONG, LONG) noexcept override {
    ++preedits; Check(ticket == last, 701); return true;
  }
  bool Payload(const TicketRef& ticket, const PayloadGrant&, uint32_t offset, const char* value,
               uint32_t n, bool) noexcept override {
    ++chunks; Check(ticket == last, 702); Check(offset == bytes.size(), 703);
    if (fail_payload) return false;
    bytes.append(value, n);
    return true;
  }
  bool Finish(const TicketRef& ticket, uint64_t) noexcept override {
    ++finishes; Check(ticket == last, 704); return true;
  }
  void Cancel(const TicketRef&, Fault value) noexcept override { ++cancels; cancel_fault = value; }
  void Faulted(const OwnerRef&, Fault value) noexcept override { ++faults; fault = value; }
  bool CoreRevoke(const OwnerRef& owner, uint64_t* fence) noexcept override {
    ++revokes; *fence = owner.context; return !busy_revoke;
  }
  bool ConfirmRetired(const OwnerRef&, uint64_t) noexcept override { ++retired; return on_confirm ? on_confirm() : true; }
  bool ContextQuiet(const OwnerRef&, const TicketRef*, uint64_t) noexcept override { ++context_quiet; return true; }
  void Pointer(const OwnerRef&, const PointerEvent& value) noexcept override { ++pointers; pointer = value; }
  Disposition disposition{Disposition::AllowNativeText};
  TicketRef last{};
  std::string bytes;
  Fault fault{};
  Fault cancel_fault{};
  PointerEvent pointer{};
  unsigned gates{}, reserves{}, preedits{}, chunks{}, finishes{}, cancels{}, faults{}, revokes{}, retired{}, pointers{};
  unsigned context_quiet{};
  bool fail_reserve{}, fail_payload{}, busy_revoke{};
  std::function<bool()> on_confirm;
  std::function<void()> on_reserve;
};

class Edit final : public ITfEditSession {
 public:
  explicit Edit(std::function<HRESULT(TfEditCookie)> fn) : fn_(std::move(fn)) {}
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITfEditSession) return E_NOINTERFACE;
    *out = static_cast<ITfEditSession*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return ++refs_; }
  ULONG STDMETHODCALLTYPE Release() override { ULONG n = --refs_; if (!n) delete this; return n; }
  HRESULT STDMETHODCALLTYPE DoEditSession(TfEditCookie cookie) override { ++calls; return fn_(cookie); }
  unsigned calls{};
 private:
  ULONG refs_{1};
  std::function<HRESULT(TfEditCookie)> fn_;
};
class CompositionSink final : public ITfCompositionSink {
 public:
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITfCompositionSink) return E_NOINTERFACE;
    *out = static_cast<ITfCompositionSink*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return ++refs_; }
  ULONG STDMETHODCALLTYPE Release() override { ULONG n = --refs_; if (!n) delete this; return n; }
  HRESULT STDMETHODCALLTYPE OnCompositionTerminated(TfEditCookie, ITfComposition*) override { ++terminated; return S_OK; }
  unsigned terminated{};
 private: ULONG refs_{1};
};
class LockSink final : public ITextStoreACPSink {
 public:
  explicit LockSink(TextStore* store) : store(store) {}
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITextStoreACPSink) return E_NOINTERFACE;
    *out = static_cast<ITextStoreACPSink*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return ++refs; }
  ULONG STDMETHODCALLTYPE Release() override { ULONG n = --refs; if (!n) delete this; return n; }
  HRESULT STDMETHODCALLTYPE OnTextChange(DWORD, const TS_TEXTCHANGE*) override {
    Check(!in_lock, 720); ++texts; return on_text ? on_text() : S_OK;
  }
  HRESULT STDMETHODCALLTYPE OnSelectionChange() override { Check(!in_lock, 721); return S_OK; }
  HRESULT STDMETHODCALLTYPE OnLayoutChange(TsLayoutCode, TsViewCookie) override { Check(!in_lock, 722); return S_OK; }
  HRESULT STDMETHODCALLTYPE OnStatusChange(DWORD) override { return S_OK; }
  HRESULT STDMETHODCALLTYPE OnAttrsChange(LONG, LONG, ULONG, const TS_ATTRID*) override { return S_OK; }
  HRESULT STDMETHODCALLTYPE OnStartEditTransaction() override { return S_OK; }
  HRESULT STDMETHODCALLTYPE OnEndEditTransaction() override { return S_OK; }
  HRESULT STDMETHODCALLTYPE OnLockGranted(DWORD flags) override {
    Check(!in_lock, 723); in_lock = true; ++locks;
    HRESULT hr = operation ? operation(flags) : S_OK;
    in_lock = false; return hr;
  }
  TextStore* store;
  ULONG refs{1};
  unsigned locks{}, texts{};
  bool in_lock{};
  std::function<HRESULT(DWORD)> operation;
  std::function<HRESULT()> on_text;
};

LRESULT CALLBACK WindowProc(HWND hwnd, UINT message, WPARAM wp, LPARAM lp) {
  auto* surface = reinterpret_cast<NativeSurface*>(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
  if (surface && surface->HandleOwnedMessage(message, wp, lp)) return 0;
  if (message == WM_CLOSE) return 0;
  return DefWindowProcW(hwnd, message, wp, lp);
}
OwnerRef Owner(uint64_t context) { return {1, 1, 1, 1, 1, context}; }
TicketRef Ticket(uint64_t context) { return {1, context, context, context}; }
SourceGeometry Geometry() {
  SourceGeometry g{};
  g.revision = {1, 1, 1, 1, 1};
  g.screen_clip = {0, 0, 640, 480}; g.cursor_screen = {20, 20, 22, 40};
  g.cell_width = 8; g.cell_height = 16; g.rows = 30; g.columns = 80; g.acknowledged = true;
  return g;
}
void RunEdit(ITfContext* context, TfClientId client, const std::function<HRESULT(TfEditCookie)>& fn,
              unsigned label) {
  ComPtr<Edit> edit; edit.Attach(new Edit(fn));
  HRESULT session = E_FAIL;
  Hr(context->RequestEditSession(client, edit.Get(), TF_ES_SYNC | TF_ES_READWRITE, &session), label);
  Hr(session, label + 1);
  Check(edit->calls == 1, label + 2);
}
HRESULT SetWhole(ITfContext* context, TfEditCookie cookie, const WCHAR* text, LONG units,
                  ITfRange** range_out = nullptr) {
  ComPtr<ITfRange> start, end;
  HRESULT hr = context->GetStart(cookie, &start);
  if (SUCCEEDED(hr)) hr = context->GetEnd(cookie, &end);
  if (SUCCEEDED(hr)) hr = start->ShiftEndToRange(cookie, end.Get(), TF_ANCHOR_END);
  if (SUCCEEDED(hr)) hr = start->SetText(cookie, 0, text, units);
  if (SUCCEEDED(hr) && range_out) *range_out = start.Detach();
  return hr;
}

void StoreContracts(HWND hwnd, int* live) {
  auto consumer = std::make_shared<Consumer>();
  auto permit = std::make_shared<Permit>(live);
  ComPtr<TextStore> store; store.Attach(TextStore::Create(hwnd, Owner(101), consumer, permit));
  ComPtr<LockSink> sink; sink.Attach(new LockSink(store.Get()));
  Hr(store->AdviseSink(IID_ITextStoreACPSink, sink.Get(), TS_AS_ALL_SINKS), 100);
  Hr(store->Bind(Ticket(101)), 101);
  Hr(store->BeginDispatch({Ticket(101), 101, 1, TextRoute::TsfStore}), 102);
  TS_TEXTCHANGE change{};
  Check(store->SetText(0, 0, 0, L"x", 1, &change) == TS_E_NOLOCK, 103);
  sink->operation = [&](DWORD flags) {
    if ((flags & TS_LF_READWRITE) == TS_LF_READ) {
      HRESULT nested{};
      Hr(store->RequestLock(TS_LF_SYNC | TS_LF_READWRITE, &nested), 104);
      Check(nested == TS_E_SYNCHRONOUS, 105);
      Hr(store->RequestLock(TS_LF_READWRITE, &nested), 106);
      Check(nested == TS_S_ASYNC, 107);
      return S_OK;
    }
    const WCHAR value[] = {L'a', 0xd83d, 0xde00, L'b'};
    Hr(store->SetText(0, 0, 0, value, 4, &change), 108);
    LONG insert_start{}, insert_end{};
    Hr(store->QueryInsert(0, 0, 20, &insert_start, &insert_end), 132);
    Check(insert_start == 0 && insert_end == 0, 133);
    TS_SELECTION_ACP selection{2, 2, {TS_AE_END, FALSE}};
    Check(store->SetSelection(1, &selection) == TS_E_INVALIDPOS, 109);
    selection = {1, 3, {TS_AE_START, FALSE}};
    Hr(store->SetSelection(1, &selection), 110);
    Hr(store->InsertTextAtSelection(TS_IAS_QUERYONLY, nullptr, 3, &insert_start, &insert_end, nullptr), 134);
    Check(insert_start == 1 && insert_end == 4, 135);
    const WCHAR pair[] = {0xd83d, 0xde00};
    Hr(store->InsertTextAtSelection(0, pair, 2, &insert_start, &insert_end, &change), 136);
    Check(change.acpStart == 1 && change.acpOldEnd == 3 && change.acpNewEnd == 3, 137);
    WCHAR output[4]{}; ULONG plain{}, runs{}; LONG next{};
    TS_RUNINFO run{};
    Hr(store->GetText(0, -1, output, 2, &plain, &run, 1, &runs, &next), 111);
    Check(plain == 1 && next == 1 && output[0] == L'a', 112);
    Check(store->GetText(2, -1, output, 4, &plain, &run, 1, &runs, &next) == TS_E_INVALIDPOS, 113);
    Hr(store->GetText(1, 3, output, 2, &plain, &run, 1, &runs, &next), 114);
    Check(plain == 2 && output[0] == 0xd83d && output[1] == 0xde00, 115);
    Check(store->GetText(1, 3, output, 1, &plain, &run, 1, &runs, &next) ==
          HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER), 138);
    RECT rect{}; BOOL clipped{};
    Check(store->GetTextExt(1, 0, 4, &rect, &clipped) == TS_E_NOLAYOUT, 116);
    store->SetGeometry(Geometry());
    const RECT rects[] = {{20,20,28,40},{28,20,44,40},{28,20,44,40},{44,20,52,40},{52,20,54,40}};
    Hr(store->SetTextLayout(rects, 5, store->Revision()), 117);
    Hr(store->GetTextExt(1, 1, 3, &rect, &clipped), 118);
    Check(rect.left == 28 && rect.right == 44 && !clipped, 119);
    POINT point{30, 30}; LONG acp{};
    Hr(store->GetACPFromPoint(1, &point, 0, &acp), 120);
    Check(acp == 1, 121);
    WCHAR bad[] = {0xd800};
    Check(store->SetText(0, 0, 4, bad, 1, &change) == E_INVALIDARG, 122);
    return S_OK;
  };
  HRESULT session{};
  Hr(store->RequestLock(TS_LF_READ | TS_LF_SYNC, &session), 123);
  Hr(session, 124); Check(sink->locks == 2, 125);
  Hr(store->EndDispatch(), 126);
  // 直接 ACP lock 不冒充真实 TSF OnEndEdit；提交覆盖见 ActualTsf。
  Check(consumer->finishes == 0 && consumer->bytes.empty() && !store->Sealed(), 127);
  Hr(store->PumpNotifications(), 128);
  Check(consumer->finishes == 0 && sink->texts == 0, 129);
  Check(FAILED(store->Bind(Ticket(102))), 130);
  Hr(store->UnadviseSink(sink.Get()), 131);
  store->Retire();
}

void ActualTsf(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool;
  auto consumer = std::make_shared<Consumer>();
  ComPtr<NativeSurface> surface;
  Hr(pool.Create(hwnd, Owner(201), consumer, std::make_shared<Permit>(live), &surface, NativeOrigin::DeclaredSynthetic), 200);
  if (!surface) return;
  Hr(surface->Open(manager, client), 201);
  if (!surface->Context()) { surface->BeginRevoke(); return; }
  Hr(surface->ActivateDocument(), 202);
  auto* store = surface->Store();
  Hr(store->Bind(Ticket(201)), 203);
  Hr(store->BeginDispatch({Ticket(201), 201, 1, TextRoute::TsfStore}), 204);
  ComPtr<ITfComposition> composition;
  ComPtr<CompositionSink> sink; sink.Attach(new CompositionSink());
  RunEdit(surface->Context(), client, [&](TfEditCookie cookie) {
    ComPtr<ITfRange> range;
    HRESULT hr = SetWhole(surface->Context(), cookie, L"abc", 3, &range);
    ComPtr<ITfContextComposition> owner;
    if (SUCCEEDED(hr)) hr = surface->Context()->QueryInterface(IID_PPV_ARGS(&owner));
    if (SUCCEEDED(hr)) hr = owner->StartComposition(cookie, range.Get(), sink.Get(), &composition);
    return hr;
  }, 205);
  Hr(store->EndDispatch(), 208);
  Check(store->Composing() && consumer->finishes == 0 && consumer->bytes.empty(), 209);
  Hr(store->BeginDispatch({Ticket(201), 201, 2, TextRoute::TsfStore}), 210);
  RunEdit(surface->Context(), client, [&](TfEditCookie cookie) {
    return SetWhole(surface->Context(), cookie, L"abcd", 4);
  }, 211);
  Hr(store->EndDispatch(), 214);
  Check(consumer->finishes == 0 && consumer->preedits >= 2, 215);
  Hr(store->BeginDispatch({Ticket(201), 201, 3, TextRoute::TsfStore}), 216);
  RunEdit(surface->Context(), client, [&](TfEditCookie cookie) {
    const WCHAR final[] = {L'a', 0xd83d, 0xde00, L'b', L'c', L'd'};
    ComPtr<ITfRange> range;
    HRESULT hr = SetWhole(surface->Context(), cookie, final, 6, &range);
    WCHAR actual[8]{}; ULONG count{};
    if (SUCCEEDED(hr)) hr = range->GetText(cookie, 0, actual, 8, &count);
    Check(count == 6 && actual[1] == 0xd83d && actual[2] == 0xde00, 234);
    if (SUCCEEDED(hr) && composition) hr = composition->EndComposition(cookie);
    return hr;
  }, 217);
  Hr(store->EndDispatch(), 220);
  Check(consumer->finishes == 1 && consumer->bytes == "a\xf0\x9f\x98\x80" "bcd", 221);
  Check(consumer->chunks == 2, 222);
  Hr(surface->Pump(), 223);
  Check(consumer->finishes == 1, 224);
  std::printf("STAT synthetic_tsf preedit=%u finish=%u chunks=%u bytes=%zu\n",
      consumer->preedits, consumer->finishes, consumer->chunks, consumer->bytes.size());
  // 外部旧 COM 引用保持 tombstone 与 permit；放开后才确认退休。
  ComPtr<TextStore> late = store;
  composition.Reset(); sink.Reset();
  consumer->busy_revoke = true;
  Check(surface->BeginRevoke() == Fault::Busy && surface->State() == FocusState::CoreRevoking, 225);
  consumer->busy_revoke = false;
  Check(surface->CoreFenceCompleted(201) == Fault::NativeRetirementPending, 226);
  Check(consumer->retired == 0, 227);
  Check(FAILED(late->BeginDispatch({Ticket(202), 202, 4, TextRoute::TsfStore})), 228);
  Hr(late->OnEndComposition(nullptr), 229);
  Hr(late->OnEndEdit(nullptr, 0, nullptr), 230);
  Check(consumer->finishes == 1, 231);
  late.Reset();
  const Fault retired = surface->TryConfirmRetired();
  Check(retired == Fault::None, 232);
  if (retired != Fault::None) std::printf("STAT synthetic_retire fault=%u refs=%lu\n", static_cast<unsigned>(retired), store->References());
  surface.Reset();
  Check(pool.Collect() == 0, 233);
}

void ActualCancel(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool;
  auto consumer = std::make_shared<Consumer>();
  ComPtr<NativeSurface> surface;
  Hr(pool.Create(hwnd, Owner(251), consumer, std::make_shared<Permit>(live), &surface, NativeOrigin::DeclaredSynthetic), 250);
  Hr(surface->Open(manager, client), 251);
  Hr(surface->ActivateDocument(), 252);
  auto* store = surface->Store();
  Hr(store->Bind(Ticket(251)), 253);
  Hr(store->BeginDispatch({Ticket(251),251,1,TextRoute::TsfStore}), 254);
  ComPtr<ITfComposition> composition;
  ComPtr<CompositionSink> sink; sink.Attach(new CompositionSink());
  RunEdit(surface->Context(), client, [&](TfEditCookie cookie) {
    ComPtr<ITfRange> range;
    HRESULT hr = SetWhole(surface->Context(), cookie, L"cancel", 6, &range);
    ComPtr<ITfContextComposition> owner;
    if (SUCCEEDED(hr)) hr = surface->Context()->QueryInterface(IID_PPV_ARGS(&owner));
    if (SUCCEEDED(hr)) hr = owner->StartComposition(cookie, range.Get(), sink.Get(), &composition);
    return hr;
  }, 255);
  Hr(store->EndDispatch(), 258);
  Check(store->Composing() && consumer->finishes == 0, 259);
  // 真实 owner composition services 终止，不伪造 OnEndComposition。
  const Fault revoke = surface->BeginRevoke();
  Check(revoke == Fault::None || revoke == Fault::NativeRetirementPending, 260);
  Check(sink->terminated == 1 && !store->Composing(), 261);
  Check(consumer->cancels == 1 && consumer->finishes == 0 && consumer->bytes.empty(), 262);
  composition.Reset(); sink.Reset();
  Check(surface->TryConfirmRetired() == Fault::None, 263);
  surface.Reset(); Check(pool.Collect() == 0, 264);
}

void ActualPlain(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool;
  std::string first;
  for (uint64_t iteration = 0; iteration < 3; ++iteration) {
    const uint64_t id = 271 + iteration;
    auto consumer = std::make_shared<Consumer>();
    ComPtr<NativeSurface> surface;
    Hr(pool.Create(hwnd, Owner(id), consumer, std::make_shared<Permit>(live), &surface, NativeOrigin::DeclaredSynthetic), 270);
    Hr(surface->Open(manager, client), 271);
    Hr(surface->ActivateDocument(), 272);
    auto* store = surface->Store();
    Hr(store->Bind(Ticket(id)), 273);
    Hr(store->BeginDispatch({Ticket(id),id,1,TextRoute::TsfStore}), 274);
    RunEdit(surface->Context(), client, [&](TfEditCookie cookie) {
      if (iteration == 2) return SetWhole(surface->Context(), cookie, L"", 0);
      const WCHAR value[] = L"0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz";
      return SetWhole(surface->Context(), cookie, value, static_cast<LONG>(std::size(value) - 1));
    }, 275);
    Hr(store->EndDispatch(), 278);
    Check(consumer->finishes == 1 && consumer->cancels == 0, 279);
    if (iteration == 0) { first = consumer->bytes; Check(first.size() > 32 && consumer->chunks > 1, 280); }
    else if (iteration == 1) Check(consumer->bytes == first, 281);
    else Check(consumer->bytes.empty() && consumer->chunks == 1, 282);
    Check(surface->BeginRevoke() == Fault::None, 283);
    surface.Reset(); Check(pool.Collect() == 0, 284);
  }
}

void ContextLocalCancel(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool;
  auto consumer = std::make_shared<Consumer>();
  ComPtr<NativeSurface> surface;
  Hr(pool.Create(hwnd, Owner(291), consumer, std::make_shared<Permit>(live), &surface,
      NativeOrigin::DeclaredSynthetic), 290);
  Hr(surface->Open(manager, client), 291);
  Hr(surface->ActivateDocument(), 292);
  auto* store = surface->Store();
  Hr(store->Bind(Ticket(291)), 293);
  Hr(store->BeginDispatch({Ticket(291), 291, 1, TextRoute::TsfStore}), 294);
  ComPtr<ITfComposition> composition;
  ComPtr<CompositionSink> sink; sink.Attach(new CompositionSink());
  RunEdit(surface->Context(), client, [&](TfEditCookie cookie) {
    ComPtr<ITfRange> range;
    HRESULT hr = SetWhole(surface->Context(), cookie, L"pending", 7, &range);
    ComPtr<ITfContextComposition> owner;
    if (SUCCEEDED(hr)) hr = surface->Context()->QueryInterface(IID_PPV_ARGS(&owner));
    if (SUCCEEDED(hr)) hr = owner->StartComposition(cookie, range.Get(), sink.Get(), &composition);
    return hr;
  }, 295);
  Hr(store->EndDispatch(), 298);
  RunEdit(surface->Context(), client, [&](TfEditCookie) {
    Check(surface->BeginContextRetirement(19) == Fault::NativeRetirementPending, 900);
    Check(!store->CanDetach() && FAILED(surface->RetirementResult()), 901);
    Check(!consumer->context_quiet && !consumer->revokes && !consumer->cancels, 902);
    return S_OK;
  }, 903);
  ComPtr<TextStore> external_store(store);
  // 原锁退出后在 UI Pump 重试；不把 TerminateComposition 返回当 quiet。
  surface->Pump();
  Check(sink->terminated == 1 && !store->Composing(), 906);
  Check(!consumer->context_quiet, 907);
  composition.Reset(); sink.Reset();
  external_store.Reset();
  for (unsigned i = 0; i < 16 && surface->State() != FocusState::Retired; ++i) surface->Pump();
  Check(surface->State() == FocusState::Retired && consumer->context_quiet == 1, 908);
  Check(!consumer->revokes && !consumer->cancels && !consumer->finishes, 909);
  surface.Reset(); Check(pool.Collect() == 0, 910);
}

void QueueAndEpoch(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool;
  for (uint64_t i = 0; i < 2; ++i) {
    auto consumer = std::make_shared<Consumer>();
    ComPtr<NativeSurface> surface;
    Hr(pool.Create(hwnd, Owner(351+i), consumer, std::make_shared<Permit>(live), &surface, NativeOrigin::DeclaredSynthetic), 350);
    Hr(surface->Open(manager, client), 351);
    Hr(surface->ActivateDocument(), 352);
    if (!i) {
      const LPARAM lp = 1 | (static_cast<LPARAM>(MapVirtualKeyW(VK_SPACE, MAPVK_VK_TO_VSC)) << 16);
      Check(PostMessageW(hwnd, WM_SYSKEYDOWN, VK_F1, lp) != FALSE, 353);
      Check(PostMessageW(hwnd, WM_CHAR, L'X', lp) != FALSE, 354);
      MSG key{hwnd, WM_KEYDOWN, VK_SPACE, lp};
      const auto result = surface->Dispatch(key, nullptr);
      Check(result.fault == Fault::AmbiguousTextRoute && result.tsf_calls == 0 && result.translates == 0, 355);
      MSG preserved{};
      Check(PeekMessageW(&preserved, hwnd, WM_SYSKEYDOWN, WM_SYSKEYDOWN, PM_REMOVE) != FALSE, 356);
      Check(preserved.wParam == VK_F1, 357);
      // 仅清理本 fixture 自己投递的确切派生消息，不作通用 drain。
      Check(PeekMessageW(&preserved, hwnd, WM_CHAR, WM_CHAR, PM_REMOVE) != FALSE, 358);
      Check(preserved.wParam == L'X', 359);
      Check(surface->BeginRevoke() == Fault::None, 360);
    } else {
      const auto before = surface->Profile();
      Hr(surface->OnActivated(before.type, before.language, before.clsid, GUID_TFCAT_TIP_KEYBOARD,
                               before.profile, before.hkl, TF_IPSINK_FLAG_ACTIVE), 361);
      Check(surface->Profile().epoch == before.epoch + 1 && surface->Profile().hkl == before.hkl, 362);
      Check(consumer->revokes == 1 && surface->State() == FocusState::Retired, 363);
    }
    surface.Reset(); Check(pool.Collect() == 0, 364);
  }
}

void PoolCapacity(HWND hwnd, int* live) {
  ContextPool pool;
  auto consumer = std::make_shared<Consumer>();
  std::array<ComPtr<NativeSurface>, kContextSlots> held;
  for (size_t i = 0; i < held.size(); ++i)
    Hr(pool.Create(hwnd, Owner(501+i), consumer, std::make_shared<Permit>(live), &held[i]), 500);
  ComPtr<NativeSurface> extra;
  Check(pool.Create(hwnd, Owner(600), consumer, std::make_shared<Permit>(live), &extra) ==
        HRESULT_FROM_WIN32(ERROR_NOT_ENOUGH_QUOTA), 501);
  Check(pool.Occupied() == kContextSlots, 502);
  for (auto& surface : held) Check(surface->BeginRevoke() == Fault::None, 503);
  Check(pool.Collect() == kContextSlots, 504);
  for (auto& surface : held) surface.Reset();
  Check(pool.Collect() == 0, 505);
  Check(pool.Create(hwnd, Owner(501), consumer, std::make_shared<Permit>(live), &extra) == E_ACCESSDENIED, 506);
}

void NativeFocusBoundary(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool;
  auto consumer = std::make_shared<Consumer>();
  ComPtr<NativeSurface> surface;
  // 默认 VerifiedNative；隐藏 fixture 未获 Win32 焦点，必须在 Gate 前拒绝。
  Hr(pool.Create(hwnd, Owner(651), consumer, std::make_shared<Permit>(live), &surface), 650);
  Hr(surface->Open(manager, client), 651);
  Check(GetFocus() != hwnd && surface->ActivateDocument() == E_ACCESSDENIED, 652);
  MSG key{hwnd, WM_KEYDOWN, VK_SPACE, 1};
  auto rejected = surface->Dispatch(key, nullptr);
  Check(rejected.owned && rejected.fault == Fault::TextContextUnverified && consumer->gates == 0, 653);
  Check(surface->State() == FocusState::Retired, 654);
  surface.Reset(); Check(pool.Collect() == 0, 655);
}

void NativeMessages(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool;
  auto consumer = std::make_shared<Consumer>();
  ComPtr<NativeSurface> surface;
  Hr(pool.Create(hwnd, Owner(301), consumer, std::make_shared<Permit>(live), &surface, NativeOrigin::DeclaredSynthetic), 300);
  Hr(surface->Open(manager, client), 301);
  Hr(surface->ActivateDocument(), 302);
  SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(surface.Get()));
  LPARAM lp = 1 | (static_cast<LPARAM>(MapVirtualKeyW(VK_SPACE, MAPVK_VK_TO_VSC)) << 16);
  MSG msg{};
  Check(PostMessageW(hwnd, WM_KEYDOWN, VK_SPACE, lp) != FALSE, 303);
  Check(PeekMessageW(&msg, hwnd, WM_KEYDOWN, WM_KEYDOWN, PM_REMOVE) != FALSE, 304);
  consumer->disposition = Disposition::ConsumedDeferred;
  auto consumed = surface->Dispatch(msg, nullptr);
  Check(consumed.owned && consumed.tsf_calls == 0 && consumed.translates == 0, 305);
  consumer->disposition = Disposition::Rejected;
  auto rejected = surface->Dispatch(msg, nullptr);
  Check(rejected.tsf_calls == 0 && rejected.translates == 0, 306);
  consumer->disposition = Disposition::TerminalDeferred;
  auto deferred = surface->Dispatch(msg, nullptr);
  Check(deferred.tsf_calls == 0 && deferred.translates == 0, 307);
  consumer->disposition = Disposition::AllowNativeText;
  MSG unknown = msg; unknown.wParam = VK_PROCESSKEY;
  auto denied = surface->Dispatch(unknown, nullptr);
  Check(denied.tsf_calls == 0 && denied.translates == 0 && denied.fault != Fault::None, 308);
  auto allowed = surface->Dispatch(msg, nullptr);
  std::printf("STAT synthetic_translate route=%u tsf=%u translate=%u derived=%u hr=%08lx fault=%u\n",
      static_cast<unsigned>(allowed.route), allowed.tsf_calls, allowed.translates, allowed.derived,
      static_cast<unsigned long>(allowed.native_result), static_cast<unsigned>(allowed.fault));
  Check(allowed.fault == Fault::None && SUCCEEDED(allowed.native_result), 309);
  std::printf("STAT synthetic_finalizer finish=%u cancel=%u fault=%u bytes=%zu\n",
      consumer->finishes, consumer->cancels, static_cast<unsigned>(consumer->cancel_fault), consumer->bytes.size());
  Check(consumer->finishes == 1 && consumer->bytes == " ", 310);
  Check(!PeekMessageW(&msg, hwnd, WM_CHAR, WM_DEADCHAR, PM_NOREMOVE), 311);
  surface->SetGeometry(Geometry());
  MSG pointer{hwnd, WM_LBUTTONDOWN, MK_SHIFT | MK_CONTROL | MK_LBUTTON, MAKELPARAM(16,32)};
  Check(surface->DispatchPointer(pointer), 312);
  Check(consumer->pointers == 1 && (consumer->pointer.modifiers & 3) == 3 &&
        consumer->pointer.row == 2 && consumer->pointer.column == 2, 313);
  Check(surface->BeginRevoke() == Fault::None, 314);
  SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
  surface.Reset(); Check(pool.Collect() == 0, 315);
}

void CancelAndPayload(HWND hwnd, int* live) {
  for (uint64_t i = 0; i < 5; ++i) {
    auto consumer = std::make_shared<Consumer>();
    consumer->fail_reserve = i == 0;
    consumer->fail_payload = i == 1;
    ComPtr<TextStore> store;
    store.Attach(TextStore::Create(hwnd, Owner(401+i), consumer, std::make_shared<Permit>(live)));
    const HRESULT bound = store->Bind(Ticket(401+i));
    if (i == 0) { Check(FAILED(bound) && consumer->cancels == 1, 400); continue; }
    Hr(bound, 401);
    Hr(store->BeginDispatch({Ticket(401+i),401+i,1,TextRoute::TranslateStore}), 402);
    if (i != 2) Hr(store->AppendTranslated(L"same", 4), 403);
    if (i == 4) { store->Cancel(Fault::TextContextUnverified); store->Cancel(Fault::TextContextUnverified); }
    const HRESULT finish = store->EndDispatch();
    if (i == 1) Check(FAILED(finish) && consumer->finishes == 0 && consumer->cancels == 1, 404);
    else if (i == 4) Check(consumer->finishes == 0 && consumer->cancels == 1, 405);
    else { Hr(finish, 406); Check(consumer->finishes == 1, 407); }
    store->Retire();
  }
}

void MixedRoute(HWND hwnd, int* live) {
  auto consumer = std::make_shared<Consumer>();
  ComPtr<TextStore> store;
  store.Attach(TextStore::Create(hwnd, Owner(451), consumer, std::make_shared<Permit>(live)));
  ComPtr<LockSink> sink; sink.Attach(new LockSink(store.Get()));
  Hr(store->AdviseSink(IID_ITextStoreACPSink, sink.Get(), TS_AS_TEXT_CHANGE), 450);
  Hr(store->Bind(Ticket(451)), 451);
  Hr(store->BeginDispatch({Ticket(451),451,1,TextRoute::TranslateStore}), 452);
  Hr(store->AppendTranslated(L"a", 1), 453);
  sink->on_text = [&]() {
    HRESULT session{};
    Hr(store->RequestLock(TS_LF_READWRITE, &session), 454);
    Check(session == TS_S_ASYNC, 455);
    return S_OK;
  };
  sink->operation = [&](DWORD) {
    TS_TEXTCHANGE change{};
    return store->SetText(0, 0, 1, L"b", 1, &change);
  };
  Check(FAILED(store->EndDispatch()), 456);
  Check(consumer->finishes == 0 && consumer->bytes.empty() && consumer->cancels == 1 &&
        consumer->cancel_fault == Fault::AmbiguousTextRoute, 457);
  Hr(store->UnadviseSink(sink.Get()), 458);
  store->Retire();
}

void PendingUpgrade(HWND hwnd, int* live) {
  auto consumer = std::make_shared<Consumer>();
  ComPtr<TextStore> store; store.Attach(TextStore::Create(hwnd, Owner(801), consumer, std::make_shared<Permit>(live)));
  ComPtr<LockSink> sink; sink.Attach(new LockSink(store.Get()));
  Hr(store->AdviseSink(IID_ITextStoreACPSink, sink.Get(), TS_AS_ALL_SINKS), 800);
  Hr(store->Bind(Ticket(801)), 811);
  Hr(store->BeginDispatch({Ticket(801),801,1,TextRoute::TsfStore}), 812);
  std::array<DWORD, 8> granted{}; size_t granted_count{};
  sink->operation = [&](DWORD flags) {
    if (granted_count < granted.size()) granted[granted_count++] = flags & TS_LF_READWRITE;
    HRESULT nested{};
    if (granted_count == 1 || granted_count == 2) {
      Hr(store->RequestLock(granted_count == 1 ? TS_LF_READ : TS_LF_READWRITE, &nested), 801);
      Check(nested == TS_S_ASYNC, 802);
    }
    if ((flags & TS_LF_READWRITE) == TS_LF_READWRITE) {
      TS_TEXTCHANGE change{};
      Hr(store->SetText(0, 0, 0, L"w", 1, &change), 813);
      Check(change.acpStart == 0 && change.acpOldEnd == 0 && change.acpNewEnd == 1, 814);
    }
    LONG end{};
    Hr(store->GetEndACP(&end), 815);
    Check(end == ((flags & TS_LF_READWRITE) == TS_LF_READWRITE ? 1 : 0), 816);
    return S_OK;
  };
  HRESULT session{};
  Hr(store->RequestLock(TS_LF_SYNC | TS_LF_READ, &session), 803); Hr(session, 804);
  Check(granted_count == 2 && granted[0] == TS_LF_READ && granted[1] == TS_LF_READ, 805);
  Hr(store->RequestLock(TS_LF_SYNC | TS_LF_READ, &session), 806); Hr(session, 807);
  Hr(store->PumpNotifications(), 808);
  size_t writes{};
  for (size_t i = 0; i < granted_count; ++i) writes += granted[i] == TS_LF_READWRITE;
  Check(granted_count == 4 && granted[0] == TS_LF_READ && granted[1] == TS_LF_READ &&
        granted[2] == TS_LF_READ && granted[3] == TS_LF_READWRITE && writes == 1, 809);
  Hr(store->EndDispatch(), 817);
  Check(consumer->finishes == 0 && consumer->bytes.empty() && !store->Sealed(), 818);
  std::printf("STAT upgrade grants=%zu writes=%zu\n", granted_count, writes);
  Hr(store->UnadviseSink(sink.Get()), 810);
  store->Retire();
}

void OversizedInput(HWND hwnd, int* live) {
  auto consumer = std::make_shared<Consumer>();
  ComPtr<TextStore> store; store.Attach(TextStore::Create(hwnd, Owner(821), consumer, std::make_shared<Permit>(live)));
  ComPtr<LockSink> sink; sink.Attach(new LockSink(store.Get()));
  Hr(store->AdviseSink(IID_ITextStoreACPSink, sink.Get(), TS_AS_ALL_SINKS), 820);
  Hr(store->Bind(Ticket(821)), 821);
  Hr(store->BeginDispatch({Ticket(821),821,1,TextRoute::TsfStore}), 822);
  std::array<WCHAR, kTextUnits * 2> valid{}; valid.fill(L'a');
  ULONG examined{}; store->ObserveUtf16Reads(&examined);
  sink->operation = [&](DWORD) {
    TS_TEXTCHANGE change{};
    Hr(store->SetText(0, 0, 0, valid.data(), kTextUnits, &change), 830);
    Check(examined == kTextUnits && change.acpNewEnd == kTextUnits, 831);
    examined = 0;
    Check(store->SetText(0, 0, kTextUnits, valid.data(), static_cast<ULONG>(valid.size()), &change) == E_OUTOFMEMORY, 823);
    Check(examined == 0 && consumer->cancel_fault == Fault::TextCapacity && consumer->cancels == 1, 824);
    return S_OK;
  };
  HRESULT session{};
  Hr(store->RequestLock(TS_LF_SYNC | TS_LF_READWRITE, &session), 825); Hr(session, 826);
  Check(FAILED(store->SelectTranslateRoute({Ticket(821),821,1,TextRoute::TranslateStore})), 829);
  Check(FAILED(store->EndDispatch()), 827);
  std::printf("STAT oversized units_read=%lu\n", examined);
  Hr(store->UnadviseSink(sink.Get()), 828);
}

void ConfirmationReentry(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  ContextPool pool; auto consumer = std::make_shared<Consumer>();
  ComPtr<NativeSurface> surface;
  Hr(pool.Create(hwnd, Owner(841), consumer, std::make_shared<Permit>(live), &surface, NativeOrigin::DeclaredSynthetic), 840);
  Hr(surface->Open(manager, client), 841);
  bool inside{};
  consumer->on_confirm = [&]() {
    Check(surface->State() == FocusState::NativeRetiring && pool.Collect() == 1, 842);
    if (!inside) {
      inside = true;
      Check(surface->Pump() == E_PENDING, 843);
      inside = false;
    }
    return false;
  };
  Check(surface->BeginRevoke() == Fault::Busy, 844);
  Check(consumer->retired == 1 && surface->State() == FocusState::NativeRetiring, 845);
  consumer->on_confirm = {};
  Check(surface->TryConfirmRetired() == Fault::None && consumer->retired == 2, 846);
  surface.Reset(); Check(pool.Collect() == 0, 847);
}

void DispatchReentry(HWND hwnd, ITfThreadMgr* manager, TfClientId client, int* live) {
  struct Probe {
    HWND hwnd; unsigned boundary; unsigned calls{};
    static void Invoke(void* data, NativeSurface* surface, unsigned point) noexcept {
      auto& probe = *static_cast<Probe*>(data);
      if (point != probe.boundary || probe.calls) return;
      ++probe.calls;
      if (point == 1) SendMessageW(probe.hwnd, WM_KILLFOCUS, 0, 0);
      else surface->Store()->Cancel(Fault::TextContextUnverified);
    }
  };
  for (unsigned boundary = 1; boundary <= 2; ++boundary) {
    ContextPool pool; auto consumer = std::make_shared<Consumer>();
    ComPtr<NativeSurface> surface;
    Hr(pool.Create(hwnd, Owner(860+boundary), consumer, std::make_shared<Permit>(live), &surface, NativeOrigin::DeclaredSynthetic), 860);
    Hr(surface->Open(manager, client), 861); Hr(surface->ActivateDocument(), 862);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(surface.Get()));
    Probe probe{hwnd,boundary}; surface->ObserveBoundary(&Probe::Invoke, &probe);
    MSG msg{hwnd,WM_KEYDOWN,VK_SPACE,1 | (static_cast<LPARAM>(MapVirtualKeyW(VK_SPACE,MAPVK_VK_TO_VSC)) << 16)};
    const auto result = surface->Dispatch(msg, nullptr);
    Check(probe.calls == 1 && result.fault != Fault::None && result.translates == 0, 863);
    Check(result.tsf_calls == (boundary == 1 ? 0u : 1u), 864);
    Check(consumer->finishes == 0 && consumer->bytes.empty(), 865);
    surface->ObserveBoundary(nullptr, nullptr);
    Check(surface->BeginRevoke() == Fault::None, 866);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    surface.Reset(); Check(pool.Collect() == 0, 867);
  }
}

struct AllocationAudit final : NativeMemory {
  void* Allocate(size_t bytes) noexcept override {
    ++attempts;
    if (!held || bytes > charge - allocated) { ++violations; return nullptr; }
    if (attempts == fail_at) return nullptr;
    void* result = ::operator new(bytes, std::nothrow);
    if (result) { allocated += bytes; peak = (std::max)(peak, allocated); }
    return result;
  }
  void Deallocate(void* pointer, size_t bytes) noexcept override {
    if (!held || charge < allocated) ++violations;
    ::operator delete(pointer);
    // 此观察点在实际内存归还之后、allocator 返回之前，不检查已释放对象。
    if (!held) ++violations;
    allocated -= bytes; ++deallocated;
    last_event = 2;
  }
  size_t charge{}, allocated{}, peak{};
  unsigned attempts{}, fail_at{}, violations{}, deallocated{}, refunds{}, last_event{};
  bool held{};
};
struct AuditedPermit final : NativePermit {
  AuditedPermit(AllocationAudit& memory, size_t bytes) noexcept : memory(memory) {
    memory.charge = bytes; memory.held = true; memory.last_event = 1;
  }
  ~AuditedPermit() override {
    if (memory.allocated) ++memory.violations;
    memory.held = false; memory.charge = 0; ++memory.refunds; memory.last_event = 3;
  }
  NativeMemory& Memory() noexcept override { return memory; }
  AllocationAudit& memory;
};
void AllocationLifetimes(HWND hwnd) {
  auto consumer = std::make_shared<Consumer>();
  for (unsigned type = 0; type < 3; ++type) {
    AllocationAudit audit;
    const size_t bound = type == 0 ? sizeof(TextStore) :
        type == 1 ? sizeof(NativeSurface) : sizeof(LayoutTable) + 512;
    auto permit = std::make_shared<AuditedPermit>(audit, bound);
    if (type == 0) {
      auto* store = TextStore::Create(hwnd, Owner(881), consumer, std::move(permit));
      Check(store != nullptr && audit.held, 880);
      if (store) store->Release();
    } else if (type == 1) {
      auto* surface = NativeSurface::Create(hwnd, Owner(882), consumer, std::move(permit), NativeOrigin::DeclaredSynthetic);
      Check(surface != nullptr && audit.held, 881);
      if (surface) surface->Release();
    } else {
      auto table = LayoutTable::Create(std::move(permit));
      Check(table != nullptr && audit.held, 882);
      std::weak_ptr<LayoutTable> weak = table;
      table.reset();
      Check(audit.held && audit.allocated != 0 && audit.refunds == 0, 883);
      weak.reset();
    }
    Check(audit.violations == 0 && audit.refunds == 1 && audit.allocated == 0 && audit.last_event == 3, 884);
    Check(audit.deallocated == (type == 2 ? 2u : 1u), 885);
    std::printf("STAT allocation type=%u peak=%zu free=%u violations=%u\n", type,audit.peak,audit.deallocated,audit.violations);
  }
  for (unsigned fail_at = 1; fail_at <= 2; ++fail_at) {
    AllocationAudit audit; audit.fail_at = fail_at;
    auto permit = std::make_shared<AuditedPermit>(audit, sizeof(LayoutTable) + 512);
    auto table = LayoutTable::Create(std::move(permit));
    Check(!table && audit.refunds == 1 && audit.allocated == 0 && audit.violations == 0, 886);
    Check(audit.attempts == fail_at && audit.deallocated == fail_at - 1, 887);
  }
  Check(!LayoutTable::Create(nullptr) && !TextStore::Create(hwnd, Owner(889), consumer, nullptr), 888);
}

void CallbackRoots(HWND hwnd, ITfThreadMgr* manager, TfClientId client) {
  {
    AllocationAudit audit;
    auto consumer = std::make_shared<Consumer>();
    ComPtr<TextStore> store;
    store.Attach(TextStore::Create(hwnd, Owner(901), consumer,
        std::make_shared<AuditedPermit>(audit, sizeof(TextStore))));
    auto* raw = store.Get();
    consumer->on_reserve = [&]() {
      store.Reset();
      Check(audit.held && audit.deallocated == 0, 900);
      raw->Retire();
      Check(audit.held && audit.deallocated == 0, 901);
    };
    Check(raw->Bind(Ticket(901)) == E_ABORT, 902);
    Check(audit.refunds == 1 && audit.deallocated == 1 && audit.violations == 0, 903);
  }
  {
    AllocationAudit audit;
    ContextPool pool;
    auto consumer = std::make_shared<Consumer>();
    ComPtr<NativeSurface> surface;
    Hr(pool.Create(hwnd, Owner(911), consumer,
        std::make_shared<AuditedPermit>(audit, sizeof(NativeSurface) + sizeof(TextStore)),
        &surface, NativeOrigin::DeclaredSynthetic), 910);
    Hr(surface->Open(manager, client), 911);
    auto* raw = surface.Get();
    consumer->on_confirm = [&]() {
      surface.Reset();
      Check(raw->State() == FocusState::NativeRetiring && audit.held && audit.deallocated == 0, 912);
      Check(raw->Pump() == E_PENDING && pool.Collect() == 1 && consumer->retired == 1, 913);
      return true;
    };
    Check(raw->BeginRevoke() == Fault::None, 914);
    Check(pool.Collect() == 0 && audit.deallocated == 2 && audit.refunds == 1 && audit.violations == 0, 915);
  }
}

void RetirementQueueReentry(ITfThreadMgr* manager, TfClientId client, int* live) {
  const int original_live = *live;
  HWND hwnd = CreateWindowExW(0, L"GuoSSHellNativeSurfaceOwnedFixture", L"synthetic retirement fixture",
      WS_OVERLAPPEDWINDOW, 0, 0, 640, 480, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
  Check(hwnd != nullptr, 920);
  if (!hwnd) return;
  {
    ContextPool pool;
    auto consumer = std::make_shared<Consumer>();
    ComPtr<NativeSurface> surface;
    Hr(pool.Create(hwnd, Owner(921), consumer, std::make_shared<Permit>(live), &surface,
                   NativeOrigin::DeclaredSynthetic), 921);
    Hr(surface->Open(manager, client), 922);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(surface.Get()));
    struct Probe {
      HWND hwnd;
      unsigned calls{};
      static void Invoke(void* data, NativeSurface*, unsigned point) noexcept {
        auto& probe = *static_cast<Probe*>(data);
        if (point != 5 || probe.calls) return;
        ++probe.calls;
        // 在退休 Peek 返回的可重入边界同步进入本 HWND；没有 Translate 或 kernel 状态变更。
        SendMessageW(probe.hwnd, WM_DEADCHAR, L'^', 1);
      }
    } probe{hwnd};
    surface->ObserveBoundary(&Probe::Invoke, &probe);
    const Fault result = surface->BeginRevoke();
    Check(probe.calls == 1 && surface->Store()->DeadPending(), 923);
    Check(result == Fault::NativeRetirementPending && consumer->retired == 0, 924);
    Check(surface->State() == FocusState::NativeRetiring && surface->Context() == nullptr, 925);
    Check(!surface->Store()->CanDetach() && surface->Store()->References() == 1, 926);
    MSG derived{};
    Check(!PeekMessageW(&derived, hwnd, WM_CHAR, WM_DEADCHAR, PM_NOREMOVE), 927);
    surface->ObserveBoundary(nullptr, nullptr);
    Check(surface->TryConfirmRetired() == Fault::NativeRetirementPending && consumer->retired == 0, 928);
    std::printf("STAT retirement_queue callbacks=%u pending=%u dead=%u\n", consumer->retired,
        surface->State() == FocusState::NativeRetiring ? 1u : 0u,
        surface->Store()->DeadPending() ? 1u : 0u);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    surface.Reset();
    Check(pool.Collect() == 1, 929);
    // 仅合成消息造成本地 flag；TSF 已解除，终结整套 run-owned fixture，绝不清 flag 或复用 source。
  }
  Check(DestroyWindow(hwnd) != FALSE, 930);
  Check(*live == original_live, 931);
}
}

int main(int argc, char** argv) {
  const bool focused = argc == 2 && std::strcmp(argv[1], "--retirement-queue-only") == 0;
  // 首次 USER/COM/window 初始化前的诊断查询，不冒充既有管理 worker 隔离证明。
  int live{};
  std::printf("STAT native_fixed store=%zu surface=%zu pool=%zu table=%zu\n",
      sizeof(TextStore), sizeof(NativeSurface), sizeof(ContextPool), sizeof(LayoutTable));
  {
    ProfileIdentity profile{}; profile.hkl = GetKeyboardLayout(0); profile.epoch = 1;
    AllocationAudit allocation;
    // fixed grant 在对象与 shared control block 的任何分配之前存在。
    auto permit = std::make_shared<AuditedPermit>(allocation, sizeof(LayoutTable) + 512);
    auto table = LayoutTable::Create(std::move(permit));
    Check(table != nullptr, 13);
    if (!table) return 1;
    Check(table->DiagnosticQuery(profile) == Fault::None, 1);
    Check(!table->Verified(), 2);
    std::printf("STAT layout cells=%zu bytes=%zu dead=%zu verified=0\n",
        kScanCount*kLayoutStates, LayoutTable::StorageBytes(), table->DeadCells());
    WorkerEvidence absent{};
    Check(table->Build(profile, absent) == Fault::LayoutStateNotIsolated, 3);
    table.reset();
    Check(allocation.violations == 0 && allocation.refunds == 1 && allocation.allocated == 0, 14);
  }
  HRESULT co = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  Hr(co, 4); if (FAILED(co)) return 1;
  {
    WNDCLASSW wc{}; wc.lpfnWndProc = WindowProc; wc.hInstance = GetModuleHandleW(nullptr);
    wc.lpszClassName = L"GuoSSHellNativeSurfaceOwnedFixture";
    const ATOM atom = RegisterClassW(&wc); Check(atom != 0, 5);
    HWND hwnd = CreateWindowExW(0, wc.lpszClassName, L"synthetic native fixture", WS_OVERLAPPEDWINDOW,
        0, 0, 640, 480, nullptr, nullptr, wc.hInstance, nullptr);
    Check(hwnd != nullptr, 6);
    ComPtr<ITfThreadMgr> manager;
    Hr(CoCreateInstance(CLSID_TF_ThreadMgr, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&manager)), 7);
    TfClientId client{};
    if (manager && hwnd) {
      const HRESULT activated = manager->Activate(&client); Hr(activated, 8);
      if (SUCCEEDED(activated)) {
        if (!focused) {
        StoreContracts(hwnd, &live);
        ActualTsf(hwnd, manager.Get(), client, &live);
        ActualCancel(hwnd, manager.Get(), client, &live);
        ContextLocalCancel(hwnd, manager.Get(), client, &live);
        ActualPlain(hwnd, manager.Get(), client, &live);
        NativeMessages(hwnd, manager.Get(), client, &live);
        QueueAndEpoch(hwnd, manager.Get(), client, &live);
        CancelAndPayload(hwnd, &live);
        MixedRoute(hwnd, &live);
        PoolCapacity(hwnd, &live);
        NativeFocusBoundary(hwnd, manager.Get(), client, &live);
        PendingUpgrade(hwnd, &live);
        OversizedInput(hwnd, &live);
        ConfirmationReentry(hwnd, manager.Get(), client, &live);
        DispatchReentry(hwnd, manager.Get(), client, &live);
        AllocationLifetimes(hwnd);
        CallbackRoots(hwnd, manager.Get(), client);
        }
        RetirementQueueReentry(manager.Get(), client, &live);
        Hr(manager->Deactivate(), 9);
      }
    }
    manager.Reset();
    if (hwnd) Check(DestroyWindow(hwnd) != FALSE, 10);
    if (atom) Check(UnregisterClassW(wc.lpszClassName, wc.hInstance) != FALSE, 11);
  }
  CoUninitialize();
  Check(live == 0, 12);
  std::printf("RESULT synthetic_checks=%d failures=%d permits=%d\n", checks, failures, live);
  std::printf("UNVERIFIED physical_ime=1 dead_retirement=1 worker_isolation=1 altgr=1 same_hub=1 ssh_bytes=1 full_surface=1\n");
  return failures ? 1 : 0;
}
