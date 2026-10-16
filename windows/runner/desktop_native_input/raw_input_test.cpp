#include "raw_input.h"
#include "../native_input_surface/native_surface.h"

#include <cstdio>
#include <string>

using namespace guosshell::desktop_native_input;
using namespace guossh::native_input;
using Microsoft::WRL::ComPtr;
int RunLayoutProbe();
namespace {
int failures{}, checks{};
void Check(bool value, const char* name) {
  ++checks;
  if (!value) { ++failures; std::printf("FAIL %s\n", name); }
}
bool Hr(HRESULT value, const char* name) {
  if (FAILED(value)) std::printf("HRESULT %s %08lx\n", name, static_cast<unsigned long>(value));
  Check(SUCCEEDED(value), name);
  return SUCCEEDED(value);
}
RAWKEYBOARD Packet(USHORT scan, USHORT vk, USHORT flags = 0) {
  RAWKEYBOARD packet{};
  packet.MakeCode = scan;
  packet.VKey = vk;
  packet.Flags = flags;
  packet.Message = flags & RI_KEY_BREAK ? WM_KEYUP : WM_KEYDOWN;
  return packet;
}
void PressContracts(HWND hwnd) {
  RawPressState state;
  ProfileIdentity profile{};
  profile.hkl = GetKeyboardLayout(0);
  profile.epoch = 1;
  std::array<BYTE, 256> initial{};
  state.Reset(initial, profile.hkl);
  NativeKey key{};
  Check(state.Decode(hwnd, Packet(0x1e, 'A'), 1, profile, false, &key) == RawStatus::Ready, "make");
  Check(!key.repeat && !key.up && key.physical == 0x1f && key.message.lParam == 0x001e0001, "make-semantics");
  Check(state.Decode(hwnd, Packet(0x1e, 'A'), 2, profile, false, &key) == RawStatus::Ready && key.repeat &&
      (key.message.lParam & (1ULL << 30)), "repeat-before-state");
  Check(state.Decode(hwnd, Packet(0x1e, 'A', RI_KEY_BREAK), 3, profile, false, &key) == RawStatus::Ready &&
      key.up && !key.repeat && (key.message.lParam & (1ULL << 31)), "break-before-state");
  Check(state.Decode(hwnd, Packet(0x1e, 'A', RI_KEY_BREAK), 4, profile, false, &key) == RawStatus::InitialStateUnknown,
      "orphan-break-not-admitted");
  Check(state.Decode(hwnd, Packet(0x38, VK_MENU, RI_KEY_E0), 5, profile, true, &key) == RawStatus::Ready,
      "right-alt-make");
  Check(key.extended && key.message.wParam == VK_MENU && key.message.message == WM_SYSKEYDOWN &&
      key.physical_state[VK_RMENU] && !key.physical_state[VK_CONTROL] && key.queue_state[VK_CONTROL],
      "compatibility-ctrl-does-not-create-physical-ctrl");
  Check(state.Decode(hwnd, Packet(0x1e, 'A'), 6, profile, true, &key) == RawStatus::Ready &&
      key.message.message == WM_SYSKEYDOWN && (key.message.lParam & (1ULL << 29)), "alt-context");
  Check(state.Decode(hwnd, Packet(0x38, VK_MENU, RI_KEY_E0 | RI_KEY_BREAK), 7, profile, true, &key) == RawStatus::Ready &&
      !key.queue_state[VK_CONTROL] && !key.physical_state[VK_CONTROL], "altgr-release");
  Check(state.Decode(hwnd, Packet(0x1d, VK_CONTROL), 8, profile, true, &key) == RawStatus::Ready &&
      key.physical_state[VK_LCONTROL], "real-control-independent");
  Check(state.Decode(hwnd, Packet(0x45, VK_PAUSE, RI_KEY_E1), 9, profile, false, &key) == RawStatus::Ready &&
      key.e1 && !key.extended && key.physical == 0x246, "e1-identity");
  Check(state.Decode(hwnd, Packet(0xff, 255), 10, profile, false, &key) == RawStatus::InvalidPacket, "overrun-rejected");
  initial['A'] = 0x80;
  state.Reset(initial, profile.hkl);
  Check(state.Decode(hwnd, Packet(0x1e, 'A'), 11, profile, false, &key) == RawStatus::InitialStateUnknown,
      "held-on-entry-not-new-make");
  initial.fill(0);
  initial[VK_CONTROL] = initial[VK_LCONTROL] = 0x80;
  state.Reset(initial, profile.hkl);
  Check(state.Decode(hwnd, Packet(0x1e, 'A'), 12, profile, false, &key) == RawStatus::InitialStateUnknown,
      "unknown-entry-modifier-not-cleared-for-other-key");
}
void RegistrationContracts(HWND own, HWND foreign) {
  RAWINPUTDEVICE device{};
  bool present{};
  Check(RawInputOwner::QueryKeyboard(&device, &present) == RawStatus::Ready && !present, "initial-query");
  RAWINPUTDEVICE foreign_device{1, 6, 0, foreign};
  Check(RegisterRawInputDevices(&foreign_device, 1, sizeof(foreign_device)) != FALSE, "foreign-register");
  SetFocus(own);
  RawInputOwner owner;
  Check(owner.Enter(own, 1) == RawStatus::ForeignOwner, "foreign-not-overwritten");
  Check(RawInputOwner::QueryKeyboard(&device, &present) == RawStatus::Ready && present &&
      device.hwndTarget == foreign && device.dwFlags == 0, "foreign-preserved");
  RAWINPUTDEVICE remove{1, 6, RIDEV_REMOVE, nullptr};
  Check(RegisterRawInputDevices(&remove, 1, sizeof(remove)) != FALSE, "test-foreign-remove");
  Check(PostMessageW(own, WM_INPUT, RIM_INPUT, 1) != FALSE, "queued-raw-cleanup-input");
  Check(owner.Enter(own, 1) == RawStatus::Ready, "own-register");
  MSG stale{};
  Check(!PeekMessageW(&stale, own, WM_INPUT, WM_INPUT, PM_NOREMOVE) && !owner.PayloadReads(), "old-focus-queue-cleaned-without-reading");
  Check(RawInputOwner::QueryKeyboard(&device, &present) == RawStatus::Ready && present &&
      device.hwndTarget == own && device.dwFlags == RIDEV_NOLEGACY, "own-nolegacy-query");
  RawInputOwner second;
  Check(second.Enter(own, 1) == RawStatus::ForeignOwner, "single-owner");
  MSG msg{}; msg.hwnd = foreign; msg.message = WM_INPUT; msg.lParam = 1;
  NativeKey key{};
  ProfileIdentity profile{}; profile.hkl = GetKeyboardLayout(0);
  Check(owner.Read(msg, 1, profile, false, &key) == RawStatus::Ignored && !owner.PayloadReads(), "foreign-payload-not-read");
  msg.hwnd = own;
  Check(owner.Read(msg, 2, profile, false, &key) == RawStatus::StaleEpoch && !owner.PayloadReads(), "old-epoch-not-read");
  SetFocus(foreign);
  Check(owner.Read(msg, 1, profile, false, &key) == RawStatus::WrongFocus && !owner.PayloadReads(), "focus-loss-not-read");
  Check(owner.Leave() == RawStatus::Ready, "own-remove");
  Check(RawInputOwner::QueryKeyboard(&device, &present) == RawStatus::Ready && !present, "legacy-registration-restored");
  SetFocus(own);
  Check(owner.Enter(own, 1) == RawStatus::StaleEpoch, "epoch-not-reused");
  Check(owner.Enter(own, 2) == RawStatus::Ready, "next-focus-epoch");
  Check(RegisterRawInputDevices(&foreign_device, 1, sizeof(foreign_device)) != FALSE, "ownership-replacement");
  Check(owner.Leave() == RawStatus::OwnershipLost, "foreign-replacement-not-removed");
  Check(!owner.RetirementReady(own), "foreign-owner-does-not-prove-legacy-restored");
  Check(RawInputOwner::QueryKeyboard(&device, &present) == RawStatus::Ready && present && device.hwndTarget == foreign,
      "replacement-preserved");
  Check(RegisterRawInputDevices(&remove, 1, sizeof(remove)) != FALSE, "replacement-test-cleanup");
  Check(owner.RetirementReady(own), "retirement-verifies-legacy-and-owned-queue");
}
struct Permit : NativePermit {
  NativeMemory& Memory() noexcept override { return memory; }
  NativeMemory memory;
};
struct Consumer : Callbacks {
  GateReceipt Gate(const OwnerRef& owner, const NativeKey& key, const TicketRef* original) noexcept override {
    ++gates;
    return {Disposition::AllowNativeText, original ? *original : TicketRef{owner.generation, owner.context, 1, 1},
        key.trusted_raw, key.up};
  }
  bool ReservePayload(const TicketRef&, uint32_t n, PayloadGrant* out) noexcept override {
    *out = {1, n, 4096}; ++reserves; payload_offset = 0; return true;
  }
  bool Preedit(const TicketRef&, uint64_t, const WCHAR*, ULONG, LONG, LONG) noexcept override { return true; }
  bool Payload(const TicketRef&, const PayloadGrant&, uint32_t offset, const char* text, uint32_t n, bool) noexcept override {
    if (offset != payload_offset) return false;
    bytes.append(text, n); payload_offset += n; return true;
  }
  bool Finish(const TicketRef&, uint64_t) noexcept override { ++finishes; return true; }
  void Cancel(const TicketRef&, Fault) noexcept override { ++cancels; }
  void Faulted(const OwnerRef&, Fault) noexcept override {}
  bool CoreRevoke(const OwnerRef&, uint64_t* fence) noexcept override { ++revokes; *fence = 1; return true; }
  bool ConfirmRetired(const OwnerRef&, uint64_t) noexcept override { return true; }
  bool ContextQuiet(const OwnerRef&, const TicketRef*, uint64_t fence) noexcept override { ++quiet; return fence == 7; }
  void Pointer(const OwnerRef&, const PointerEvent&) noexcept override {}
  int gates{}, reserves{}, finishes{}, cancels{}, revokes{}, quiet{};
  uint32_t payload_offset{};
  std::string bytes;
};
class Edit final : public ITfEditSession {
 public:
  explicit Edit(ITfContext* context) : context_(context) {}
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
    if (!out) return E_POINTER;
    *out = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITfEditSession) return E_NOINTERFACE;
    *out = static_cast<ITfEditSession*>(this); AddRef(); return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override { return ++refs_; }
  ULONG STDMETHODCALLTYPE Release() override { const auto n = --refs_; if (!n) delete this; return n; }
  HRESULT STDMETHODCALLTYPE DoEditSession(TfEditCookie cookie) override {
    ComPtr<ITfRange> start;
    HRESULT hr = context_->GetStart(cookie, &start);
    if (SUCCEEDED(hr)) hr = start->SetText(cookie, 0, L"ACP", 3);
    return hr;
  }
 private:
  ULONG refs_{1};
  ComPtr<ITfContext> context_;
};
void NativeContracts(HWND hwnd) {
  ComPtr<ITfThreadMgr> manager;
  if (!Hr(CoCreateInstance(CLSID_TF_ThreadMgr, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&manager)), "tsf-create")) return;
  TfClientId client{};
  if (!Hr(manager->Activate(&client), "tsf-activate")) return;
  auto consumer = std::make_shared<Consumer>();
  auto permit = std::make_shared<Permit>();
  ComPtr<NativeSurface> surface;
  surface.Attach(NativeSurface::Create(hwnd, {1, 1, 1, 1, 1, 1}, consumer, permit, NativeOrigin::VerifiedNative));
  SetFocus(nullptr);
  if (!Hr(surface->Open(manager.Get(), client), "document-context-push-associate")) return;
  SetFocus(hwnd);
  Hr(surface->ActivateDocument(), "document-focus");
  RawPressState press;
  std::array<BYTE, 256> initial{};
  press.Reset(initial, surface->Profile().hkl);
  NativeKey key{};
  Check(press.Decode(hwnd, Packet(0x39, VK_SPACE), 1, surface->Profile(), false, &key) == RawStatus::Ready, "canonical-test-packet");
  const auto legacy = surface->Dispatch(key.message, nullptr);
  Check(!legacy.tsf_calls && !consumer->gates && legacy.fault == Fault::TextContextUnverified,
      "verified-surface-rejects-legacy-second-path");
  const auto dispatch = surface->DispatchCanonical(key, nullptr);
  Check(dispatch.owned && dispatch.tsf_calls > 0, "canonical-real-keystroke-manager");
  surface->Pump();
  Check(dispatch.translates == 1 && dispatch.derived == 1 && consumer->bytes == " " && consumer->finishes == 1,
      "canonical-real-tsf-translate-acp-fallback");
  surface->BeginContextRetirement(7);
  for (int i = 0; i < 16 && surface->State() != FocusState::Retired; ++i) surface->Pump();
  Check(surface->State() == FocusState::Retired && consumer->revokes == 0, "context-local-quiet-no-surface-revoke");
  surface.Reset();
  surface.Attach(NativeSurface::Create(hwnd, {1, 1, 1, 1, 1, 2}, consumer, permit, NativeOrigin::VerifiedNative));
  Hr(surface->Open(manager.Get(), client), "next-context-open");
  Hr(surface->ActivateDocument(), "next-context-focus");
  Check(press.Decode(hwnd, Packet(0x39, VK_SPACE), 2, surface->Profile(), false, &key) == RawStatus::Ready && key.repeat,
      "same-press-repeat-after-context-rotation");
  const auto repeated = surface->DispatchCanonical(key, nullptr);
  Check(repeated.translates == 1 && consumer->finishes == 2 && consumer->bytes == "  ", "repeat-new-context-new-text");
  Check(press.Decode(hwnd, Packet(0x39, VK_SPACE, RI_KEY_BREAK), 3, surface->Profile(), false, &key) == RawStatus::Ready,
      "sealed-context-up-packet");
  const auto up = surface->DispatchCanonical(key, nullptr);
  Check(up.tsf_calls == 0, "sealed-up-only-releases-press");
  Check(surface->BeginContextRetirement(7) == Fault::None, "repeat-context-quiet");
  surface.Reset();
  surface.Attach(NativeSurface::Create(hwnd, {1, 1, 1, 1, 1, 3}, consumer, permit, NativeOrigin::VerifiedNative));
  Hr(surface->Open(manager.Get(), client), "asynchronous-context-open");
  Hr(surface->ActivateDocument(), "asynchronous-context-focus");
  consumer->bytes.clear(); consumer->finishes = 0;
  auto* store = surface->Store();
  const TicketRef ticket{1, 3, 3, 3};
  Hr(store->Bind(ticket), "text-ticket");
  Hr(store->BeginDispatch({ticket, 3, 4, TextRoute::TsfStore}), "text-dispatch");
  Hr(store->EndDispatch(), "dispatch-without-edit");
  Check(!consumer->finishes && !store->Sealed(), "no-premature-empty-commit");
  Check(surface->BeginContextRetirement(0) == Fault::InvalidIdentity && surface->State() == FocusState::Active,
      "open-text-requires-original-end-fence");
  ComPtr<Edit> edit; edit.Attach(new Edit(surface->Context()));
  HRESULT session = E_FAIL;
  Hr(surface->Context()->RequestEditSession(client, edit.Get(), TF_ES_SYNC | TF_ES_READWRITE, &session), "real-edit-request");
  Hr(session, "real-acp-edit");
  edit.Reset();
  Hr(surface->Pump(), "real-onendedit-pump");
  Check(consumer->finishes == 1 && consumer->bytes == "ACP", "real-acp-complete-payload");
  surface->Pump();
  Check(consumer->finishes == 1, "real-edit-exactly-once");
  ComPtr<TextStore> external(store);
  Check(surface->BeginContextRetirement(7) == Fault::NativeRetirementPending, "external-com-reference-keeps-context");
  external.Reset();
  for (int i = 0; i < 16 && surface->State() != FocusState::Retired; ++i) surface->Pump();
  Check(surface->State() == FocusState::Retired && consumer->revokes == 0, "external-release-quiet");
  surface.Reset();
  Hr(manager->Deactivate(), "tsf-deactivate");
}
}
int main() {
  if (!Hr(CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED), "com")) return 1;
  HWND own = CreateWindowExW(0, L"STATIC", L"", WS_POPUP, 0, 0, 64, 64, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
  HWND other = CreateWindowExW(0, L"STATIC", L"", WS_POPUP, 0, 0, 64, 64, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
  if (!own || !other) return 1;
  ShowWindow(own, SW_SHOW);
  SetForegroundWindow(own);
  Check(GetForegroundWindow() == own, "owned-foreground-domain");
  PressContracts(own);
  RegistrationContracts(own, other);
  NativeContracts(own);
  Check(RunLayoutProbe() == 0, "installed-layout-worker-api");
  SetFocus(nullptr);
  DestroyWindow(other); DestroyWindow(own);
  CoUninitialize();
  std::printf("checks=%d failures=%d; API/constructed packet coverage only; physical/TIP unverified\n", checks, failures);
  return failures ? 1 : 0;
}
