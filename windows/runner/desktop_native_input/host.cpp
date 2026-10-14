#include "host_internal.h"

#include <algorithm>
#include <array>
#include <new>

namespace guosshell::desktop_native_input {
std::shared_ptr<Permit> Permit::Acquire(const std::shared_ptr<MemoryBudget>& budget) {
  const auto previous = budget->permits.fetch_add(1);
  if (previous >= 64) { --budget->permits; return {}; }
  auto* permit = new (std::nothrow) Permit(budget);
  if (!permit) { --budget->permits; return {}; }
  try { return std::shared_ptr<Permit>(permit); }
  catch (...) { return {}; }
}
Permit::~Permit() { --budget_->permits; }
void* Permit::Allocate(size_t bytes) noexcept {
  const auto previous = bytes_.fetch_add(bytes);
  if (bytes > 256 * 1024 || previous > 256 * 1024 - bytes) { bytes_ -= bytes; return nullptr; }
  void* pointer = ::operator new(bytes, std::nothrow);
  if (!pointer) bytes_ -= bytes;
  return pointer;
}
void Permit::Deallocate(void* pointer, size_t bytes) noexcept {
  ::operator delete(pointer);
  bytes_ -= bytes;
}
bool LayoutWorker::Start() {
  if (thread_) return !stopping_;
  ui_thread_ = GetCurrentThreadId();
  wake_ = CreateEventW(nullptr, FALSE, FALSE, nullptr);
  if (!wake_) return false;
  thread_ = CreateThread(nullptr, 0, Run, this, 0, nullptr);
  if (!thread_) { CloseHandle(wake_); wake_ = nullptr; return false; }
  return true;
}
DWORD WINAPI LayoutWorker::Run(void* object) {
  auto& self = *static_cast<LayoutWorker*>(object);
  while (!self.stopping_) {
    if (WaitForSingleObject(self.wake_, INFINITE) != WAIT_OBJECT_0 || self.stopping_) break;
    std::shared_ptr<ni::LayoutTable> table;
    ni::ProfileIdentity profile{};
    {
      std::lock_guard<std::mutex> lock(self.mutex_);
      table = std::move(self.pending_); profile = self.requested_;
    }
    if (table && table->QueryCanonical(profile, self.ui_thread_) != ni::Fault::None) table.reset();
    {
      std::lock_guard<std::mutex> lock(self.mutex_);
      self.ready_ = std::move(table); self.working_ = false;
    }
  }
  return 0;
}
bool LayoutWorker::Request(ni::ProfileIdentity profile, std::shared_ptr<ni::LayoutTable> table) {
  std::unique_lock<std::mutex> lock(mutex_, std::try_to_lock);
  if (!lock || stopping_ || working_ || !thread_ || !table) return false;
  requested_ = profile; pending_ = std::move(table); working_ = true;
  return SetEvent(wake_) != FALSE;
}
std::shared_ptr<const ni::LayoutTable> LayoutWorker::Result(const ni::ProfileIdentity& profile) {
  std::unique_lock<std::mutex> lock(mutex_, std::try_to_lock);
  if (!lock || !ready_ || !ni::SameProfile(ready_->Profile(), profile)) return {};
  return ready_;
}
void LayoutWorker::Stop() { stopping_ = true; if (wake_) SetEvent(wake_); }
bool LayoutWorker::Stopped() const { return !thread_ || WaitForSingleObject(thread_, 0) == WAIT_OBJECT_0; }
LayoutWorker::~LayoutWorker() {
  // Host 仅在 Stopped 后析构；异常退出保留整个 Host，不拆掉在途 worker 的存储。
  if (thread_) CloseHandle(thread_);
  if (wake_) CloseHandle(wake_);
}
std::string Utf8(const WCHAR* text, size_t count) {
  if (!count) return {};
  if (count > ni::kTextUnits) return {};
  const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text, static_cast<int>(count), nullptr, 0, nullptr, nullptr);
  if (!size) return {};
  std::string result(static_cast<size_t>(size), '\0');
  if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, text, static_cast<int>(count), result.data(), size, nullptr, nullptr) != size) return {};
  return result;
}
std::string Token() {
  GUID guid{};
  if (FAILED(CoCreateGuid(&guid))) return {};
  WCHAR text[40]{};
  const int count = StringFromGUID2(guid, text, 40);
  return count > 1 ? Utf8(text, static_cast<size_t>(count - 1)) : std::string{};
}
void NativeHostState::Emit(const char* method, Map value) noexcept {
  try { if (notify && !detached) notify(method, std::move(value)); } catch (...) { detached = true; }
}
LRESULT CALLBACK NativeHostState::WakeProc(HWND window, UINT message, WPARAM wp, LPARAM lp) {
  auto* self = reinterpret_cast<NativeHostState*>(GetWindowLongPtrW(window, GWLP_USERDATA));
  if (message == WM_NCCREATE) {
    self = static_cast<NativeHostState*>(reinterpret_cast<CREATESTRUCTW*>(lp)->lpCreateParams);
    SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(self));
  }
  if (self && message == WM_TIMER) { self->Poll(); return 0; }
  return DefWindowProcW(window, message, wp, lp);
}
bool NativeHostState::Initialize() {
  if (manager) return true;
  if (thread != GetCurrentThreadId() || !IsWindow(view)) return false;
  if (FAILED(CoCreateInstance(CLSID_TF_ThreadMgr, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&manager))) ||
      FAILED(manager->Activate(&client))) { manager.Reset(); return false; }
  WNDCLASSW wc{}; wc.hInstance = GetModuleHandleW(nullptr); wc.lpfnWndProc = WakeProc;
  wc.lpszClassName = L"GuoSSHell.NormalInput.Pump.V1";
  WNDCLASSW existing{};
  if (!RegisterClassW(&wc) && (!GetClassInfoW(wc.hInstance, wc.lpszClassName, &existing) || existing.lpfnWndProc != WakeProc)) {
    manager->Deactivate(); manager.Reset(); return false;
  }
  wake = CreateWindowExW(0, wc.lpszClassName, L"", 0, 0, 0, 0, 0, HWND_MESSAGE, nullptr, wc.hInstance, this);
  if (!wake || !SetTimer(wake, 1, 16, nullptr) || !worker.Start()) {
    if (wake) DestroyWindow(wake);
    wake = nullptr; manager->Deactivate(); manager.Reset(); return false;
  }
  return true;
}
bool NativeHostState::NativeQuiet(const PaneRoot* except) const {
  for (const auto& entry : roots) {
    const auto& root = entry.second;
    if (root.get() != except && (root->active_ || root->revoking_) && !root->quiet_) return false;
  }
  return true;
}
void NativeHostState::Poll() {
  if (pumping || thread != GetCurrentThreadId()) return;
  pumping = true;
  try {
    std::array<std::shared_ptr<PaneRoot>, kRootLimit> snapshot{};
    size_t count{};
    for (const auto& pair : roots) snapshot[count++] = pair.second;
    for (size_t i = 0; i < count; ++i) snapshot[i]->Poll();
    for (size_t i = 0; i < count; ++i) {
      const auto& root = snapshot[i];
      if (root->want_focus_ && !closing && NativeQuiet(root.get())) root->Focus();
      if (root->released_) roots.erase(root->token_);
    }
    if (closing && roots.empty()) worker.Stop();
  } catch (...) {
    for (const auto& pair : roots) pair.second->Seal("HOST_ALLOCATION_FAILED");
  }
  pumping = false;
  auto callback = poll_callback;
  if (callback) callback();
}

struct Host::Impl : NativeHostState {};
Host::Host(HWND view, const NormalApi& api, Notify notify) : impl_(std::make_unique<Impl>()) {
  impl_->view = view; impl_->api = api; impl_->notify = std::move(notify);
}
Host::~Host() {
  if (!Closed()) { impl_->notify = {}; impl_->detached = true; impl_.release(); return; }
  if (impl_->wake) { KillTimer(impl_->wake, 1); DestroyWindow(impl_->wake); }
  if (impl_->manager) impl_->manager->Deactivate();
}
void Host::Pump() { impl_->Poll(); }
bool Host::Closed() const { return impl_->roots.empty() && impl_->worker.Stopped(); }
bool Host::Close() {
  impl_->closing = true;
  for (const auto& pair : impl_->roots) { pair.second->want_release_ = true; pair.second->Seal(); }
  impl_->Poll();
  return Closed();
}
void Host::DetachChannel() {
  impl_->poll_callback = {};
  impl_->detached = true; impl_->notify = {};
  for (const auto& pair : impl_->roots) {
    auto& root = *pair.second;
    if (root.claim_) {
      auto ack = Abi<GuosshNormalAck>(); ack.claim = root.claim_; ack.kind = 2;
      auto context = root.Context();
      if (impl_->api.ack(&context, &ack) == GUOSSH_NORMAL_OK) root.claim_ = 0;
    }
  }
  Close();
}
void Host::SetPollCallback(std::function<void()> callback) { impl_->poll_callback = std::move(callback); }
void Host::FailPane(const std::string& token, const char* reason) {
  const auto found = impl_->roots.find(token);
  if (found != impl_->roots.end()) found->second->Fail(reason);
}
void Host::InvalidateAll() {
  for (const auto& pair : impl_->roots) pair.second->Invalidate();
}
void Host::Deactivate() {
  for (const auto& pair : impl_->roots) if (pair.second->active_ || pair.second->want_focus_) pair.second->Seal();
  impl_->Poll();
}
Map Host::Call(const std::string& method, const Map& args) {
  auto& h = *impl_;
  if (h.thread != GetCurrentThreadId() || !Version(args)) return State("failed");
  if (method == "attachPane") {
    const auto* source = Object(args, "source");
    const auto* identity = String(args, "identity");
    const auto* geometry = Object(args, "geometry");
    int64_t session{}; uint64_t generation{}, attempt{}, epoch{}, revision{}; Geometry parsed{};
    if (h.closing || h.roots.size() >= kRootLimit || !source || !identity || identity->empty() || identity->size() > 4096 ||
        !geometry || !Geometry::Parse(*geometry, &parsed) || !Integer(*source, "sessionId", &session) || session < 0 || session > UINT32_MAX ||
        !U64(*source, "generation", &generation) || !generation || !U64(args, "attempt", &attempt) || !attempt ||
        !U64(args, "focusEpoch", &epoch) || !U64(args, "lifecycleRevision", &revision) || !revision ||
        h.root_counter == UINT64_MAX || !h.Initialize()) return State("failed");
    for (const auto& item : h.roots)
      if (item.second->session_ == static_cast<uint32_t>(session) && item.second->generation_ == generation &&
          item.second->attempt_ == attempt && !item.second->quiet_) return State("failed");
    auto token = Token();
    if (token.empty()) return State("failed");
    auto permit = Permit::Acquire(h.budget);
    if (!permit) return State("failed");
    auto root = std::allocate_shared<PaneRoot>(ni::PermitAllocator<PaneRoot>(permit), h, ++h.root_counter,
        token, static_cast<uint32_t>(session), generation, *identity, attempt, revision);
    h.roots.emplace(token, root);
    if (!root->Create(parsed)) { root->want_release_ = true; root->Seal("PANE_CREATE_FAILED"); return State("failed"); }
    return root->Reply("attached");
  }
  const auto* token = String(args, "paneToken");
  if (!token || token->size() > 64) return State("failed");
  const auto it = h.roots.find(*token);
  if (it == h.roots.end()) return State("failed");
  auto root = it->second;
  if (method == "requestFocus" || method == "retirePane") {
    uint64_t request{};
    if (!U64(args, "requestId", &request) || !request || request <= root->last_request_) return root->Reply("failed");
    if (root->request_) root->NotifyState("failed");
    root->request_ = root->last_request_ = request;
    root->deadline_ = GetTickCount64() + kTransitionMilliseconds;
    if (method == "requestFocus") {
      if (h.closing || root->revoking_ || root->quiet_ || root->failed_) return root->Reply("failed");
      for (const auto& pair : h.roots) if (pair.second != root) {
        if (pair.second->want_focus_) { pair.second->want_focus_ = false; pair.second->NotifyState("failed"); }
        if (pair.second->active_) pair.second->Seal();
      }
      root->want_focus_ = true;
      if (h.NativeQuiet(root.get())) root->Focus();
      return root->Reply(root->Active() ? "active" : root->failed_ ? "failed" : "pending");
    }
    const auto* phase = String(args, "phase");
    if (!phase || (*phase != "quiet" && *phase != "release")) return root->Reply("failed");
    root->failed_ = false; root->failure_.clear();
    root->want_release_ = *phase == "release";
    root->release_deadline_ = 0;
    root->Seal(); root->Poll();
    const auto result = root->Reply(root->released_ ? "released" : root->quiet_ && !root->want_release_ ? "nativequiet" : root->failed_ ? "failed" : "pending");
    if (root->released_) h.roots.erase(*token);
    return result;
  }
  if (method == "presentedFrame") return root->Present(args);
  if (method == "invalidateGeometry") return root->Reply(root->Invalidate() ? "invalidated" : "failed");
  if (method == "textEndRequired") return root->TextEnd(args);
  if (method == "textLayout") return root->TextLayout(args);
  if (method == "claimAction") { uint64_t effect{}; return U64(args, "effect", &effect) ? root->Claim(effect) : root->Reply("failed"); }
  if (method == "completeAction") return root->Complete(args);
  return root->Reply("failed");
}
bool Host::BeforeTranslate(const MSG& message) {
  auto& h = *impl_;
  const bool key = TextMessage(message.message);
  std::shared_ptr<PaneRoot> own;
  for (const auto& pair : h.roots) if (pair.second->hwnd_ == message.hwnd) { own = pair.second; break; }
  if (own && message.message == WM_INPUT) { own->Raw(message); return true; }
  h.Poll();
  if (own) {
    if (key) { own->LegacyMessage(message); return true; }
    switch (message.message) {
      case WM_MOUSEMOVE: case WM_LBUTTONDOWN: case WM_LBUTTONUP: case WM_LBUTTONDBLCLK:
      case WM_MBUTTONDOWN: case WM_MBUTTONUP: case WM_MBUTTONDBLCLK:
      case WM_RBUTTONDOWN: case WM_RBUTTONUP: case WM_RBUTTONDBLCLK:
      case WM_XBUTTONDOWN: case WM_XBUTTONUP: case WM_XBUTTONDBLCLK:
      case WM_MOUSEWHEEL: case WM_MOUSEHWHEEL: own->PointerMessage(message); return true;
    }
  }
  // 不读取其他 editable 的正文；旧原生 context 未 quiet 时只阻止文本分发。
  return key && !h.NativeQuiet();
}
}  // namespace guosshell::desktop_native_input
