#include "tsf_probe.h"

#include <olectl.h>

#include <new>
#include <limits>

namespace guosshell::native_input_spike {
namespace {
volatile LONG live_stores = 0;

template <typename T>
void ReleaseObject(T*& object) noexcept {
  T* detached = object;
  object = nullptr;
  if (detached) detached->Release();
}

class AcpCallReference final {
 public:
  explicit AcpCallReference(ITextStoreACP& store) noexcept : store_(store) {
    store_.AddRef();
  }
  ~AcpCallReference() noexcept { store_.Release(); }
  AcpCallReference(const AcpCallReference&) = delete;
  AcpCallReference& operator=(const AcpCallReference&) = delete;

 private:
  ITextStoreACP& store_;
};
}  // namespace

// 有限 Wave A 探针：COM 身份、订阅和 HWND 查询真实实现；所有 ACP 编辑、
// 锁、selection、attributes／geometry 返回 Unsupported，不伪造空文本成功。
class UnsupportedTextStore final : public ITextStoreACP {
 public:
  UnsupportedTextStore(HWND hwnd, ContextIdentity identity) noexcept
      : hwnd_(hwnd), identity_(identity) {}

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** object) override {
    if (!object) return E_POINTER;
    *object = nullptr;
    if (iid != IID_IUnknown && iid != __uuidof(ITextStoreACP)) {
      return E_NOINTERFACE;
    }
    *object = static_cast<ITextStoreACP*>(this);
    AddRef();
    return S_OK;
  }
  ULONG STDMETHODCALLTYPE AddRef() override {
    return static_cast<ULONG>(InterlockedIncrement(&references_));
  }
  ULONG STDMETHODCALLTYPE Release() override {
    const LONG count = InterlockedDecrement(&references_);
    if (!count) delete this;
    return static_cast<ULONG>(count);
  }
  HRESULT STDMETHODCALLTYPE AdviseSink(REFIID iid, IUnknown* object,
                                       DWORD mask) override {
    if (!object) return E_INVALIDARG;
    if (iid != __uuidof(ITextStoreACPSink)) return E_NOINTERFACE;
    if (!active_) return CO_E_OBJNOTCONNECTED;
    // 候选 QI／Release 可释放调用方引用；本对象保留到所有成员访问结束。
    const AcpCallReference call(*this);
    const uint64_t revision = association_revision_;
    IUnknown* identity = nullptr;
    HRESULT hr = object->QueryInterface(IID_PPV_ARGS(&identity));
    if (SUCCEEDED(hr)) hr = AssociationState(revision);
    if (FAILED(hr)) {
      ReleaseObject(identity);
      return hr;
    }
    if (sink_identity_) {
      const bool same = identity == sink_identity_;
      if (!same) {
        ReleaseObject(identity);
        return CONNECT_E_ADVISELIMIT;
      }
      if (!AdvanceAssociation()) {
        ReleaseObject(identity);
        return E_UNEXPECTED;
      }
      sink_mask_ = mask;
      const uint64_t updated = association_revision_;
      ReleaseObject(identity);
      return AssociationState(updated);
    }
    ITextStoreACPSink* sink = nullptr;
    hr = object->QueryInterface(IID_PPV_ARGS(&sink));
    if (SUCCEEDED(hr)) hr = AssociationState(revision);
    if (SUCCEEDED(hr) && !AdvanceAssociation()) hr = E_UNEXPECTED;
    if (FAILED(hr)) {
      ReleaseObject(sink);
      ReleaseObject(identity);
      return hr;
    }
    sink_identity_ = identity;
    sink_ = sink;
    sink_mask_ = mask;
    return S_OK;
  }
  HRESULT STDMETHODCALLTYPE UnadviseSink(IUnknown* object) override {
    if (!object) return E_INVALIDARG;
    const AcpCallReference call(*this);
    const uint64_t revision = association_revision_;
    IUnknown* identity = nullptr;
    HRESULT hr = object->QueryInterface(IID_PPV_ARGS(&identity));
    if (SUCCEEDED(hr) && revision != association_revision_) {
      hr = active_ ? E_ABORT : CO_E_OBJNOTCONNECTED;
    }
    if (FAILED(hr)) {
      ReleaseObject(identity);
      return hr;
    }
    const bool same = identity == sink_identity_;
    if (!same) {
      ReleaseObject(identity);
      return CONNECT_E_NOCONNECTION;
    }
    DetachSink();
    ReleaseObject(identity);
    return S_OK;
  }
  HRESULT STDMETHODCALLTYPE RequestLock(DWORD, HRESULT* session) override {
    if (!session) return E_POINTER;
    *session = Unsupported();
    return *session;
  }
  HRESULT STDMETHODCALLTYPE GetStatus(TS_STATUS* status) override {
    if (!status) return E_POINTER;
    *status = {};
    if (!active_) return CO_E_OBJNOTCONNECTED;
    status->dwDynamicFlags = TS_SD_READONLY;
    status->dwStaticFlags = TS_SS_NOHIDDENTEXT;
    return S_OK;
  }
  HRESULT STDMETHODCALLTYPE QueryInsert(LONG, LONG, ULONG, LONG* start,
                                        LONG* end) override {
    if (start) *start = 0;
    if (end) *end = 0;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetSelection(ULONG, ULONG, TS_SELECTION_ACP*,
                                         ULONG* fetched) override {
    if (fetched) *fetched = 0;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE SetSelection(ULONG,
                                         const TS_SELECTION_ACP*) override {
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetText(LONG, LONG, WCHAR*, ULONG, ULONG* chars,
                                    TS_RUNINFO*, ULONG, ULONG* runs,
                                    LONG* next) override {
    if (chars) *chars = 0;
    if (runs) *runs = 0;
    if (next) *next = 0;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE SetText(DWORD, LONG, LONG, const WCHAR*, ULONG,
                                    TS_TEXTCHANGE* change) override {
    if (change) *change = {};
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetFormattedText(LONG, LONG,
                                            IDataObject** object) override {
    if (object) *object = nullptr;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetEmbedded(LONG, REFGUID, REFIID,
                                       IUnknown** object) override {
    if (object) *object = nullptr;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE QueryInsertEmbedded(const GUID*, const FORMATETC*,
                                                BOOL* insertable) override {
    if (insertable) *insertable = FALSE;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE InsertEmbedded(DWORD, LONG, LONG, IDataObject*,
                                          TS_TEXTCHANGE* change) override {
    if (change) *change = {};
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE InsertTextAtSelection(DWORD, const WCHAR*, ULONG,
                                                 LONG* start, LONG* end,
                                                 TS_TEXTCHANGE* change) override {
    if (start) *start = 0;
    if (end) *end = 0;
    if (change) *change = {};
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE InsertEmbeddedAtSelection(DWORD, IDataObject*,
                                                     LONG* start, LONG* end,
                                                     TS_TEXTCHANGE* change) override {
    if (start) *start = 0;
    if (end) *end = 0;
    if (change) *change = {};
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE RequestSupportedAttrs(DWORD, ULONG,
                                                 const TS_ATTRID*) override {
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE RequestAttrsAtPosition(LONG, ULONG,
                                                  const TS_ATTRID*, DWORD) override {
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE RequestAttrsTransitioningAtPosition(
      LONG, ULONG, const TS_ATTRID*, DWORD) override {
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE FindNextAttrTransition(LONG, LONG, ULONG,
                                                  const TS_ATTRID*, DWORD,
                                                  LONG* next, BOOL* found,
                                                  LONG* offset) override {
    if (next) *next = 0;
    if (found) *found = FALSE;
    if (offset) *offset = 0;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE RetrieveRequestedAttrs(ULONG, TS_ATTRVAL*,
                                                  ULONG* fetched) override {
    if (fetched) *fetched = 0;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetEndACP(LONG* end) override {
    if (end) *end = 0;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetActiveView(TsViewCookie* view) override {
    if (!view) return E_POINTER;
    *view = 0;
    return active_ ? S_OK : CO_E_OBJNOTCONNECTED;
  }
  HRESULT STDMETHODCALLTYPE GetACPFromPoint(TsViewCookie, const POINT*, DWORD,
                                           LONG* acp) override {
    if (acp) *acp = 0;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetTextExt(TsViewCookie, LONG, LONG, RECT* rect,
                                      BOOL* clipped) override {
    if (rect) *rect = {};
    if (clipped) *clipped = TRUE;
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetScreenExt(TsViewCookie, RECT* rect) override {
    if (rect) *rect = {};
    return Unsupported();
  }
  HRESULT STDMETHODCALLTYPE GetWnd(TsViewCookie view, HWND* hwnd) override {
    if (!hwnd) return E_POINTER;
    *hwnd = nullptr;
    if (!active_) return CO_E_OBJNOTCONNECTED;
    if (view != 0) return E_INVALIDARG;
    *hwnd = hwnd_;
    return S_OK;
  }

  void Stop() noexcept {
    if (!active_) return;
    active_ = false;
    AdvanceAssociation();
  }
  LONG References() const noexcept {
    return InterlockedCompareExchange(&references_, 0, 0);
  }

 private:
  ~UnsupportedTextStore() {
    Stop();
    DetachSink();
    InterlockedDecrement(&live_stores);
  }
  HRESULT AssociationState(uint64_t revision) const noexcept {
    if (!active_) return CO_E_OBJNOTCONNECTED;
    return revision == association_revision_ ? S_OK : E_ABORT;
  }
  bool AdvanceAssociation() noexcept {
    if (association_revision_ == (std::numeric_limits<uint64_t>::max)()) {
      active_ = false;
      return false;
    }
    ++association_revision_;
    return true;
  }
  void DetachSink() noexcept {
    // 两个关联引用与 mask 先同时离开成员，外部 Release 可安全重入新订阅。
    ITextStoreACPSink* sink = sink_;
    IUnknown* identity = sink_identity_;
    sink_ = nullptr;
    sink_identity_ = nullptr;
    sink_mask_ = 0;
    AdvanceAssociation();
    ReleaseObject(sink);
    ReleaseObject(identity);
  }
  HRESULT Unsupported() noexcept {
    // 老对象不保存 WindowSource／bridge 指针，也不重新贴 current owner。
    return active_ ? E_NOTIMPL : CO_E_OBJNOTCONNECTED;
  }
  mutable volatile LONG references_ = 1;
  bool active_ = true;
  const HWND hwnd_;
  const ContextIdentity identity_;
  IUnknown* sink_identity_ = nullptr;
  ITextStoreACPSink* sink_ = nullptr;
  DWORD sink_mask_ = 0;
  uint64_t association_revision_ = 0;
};

TsfProbe::~TsfProbe() {
  StopAdmissions();
  ReleaseSystemObjects();
  ReleaseObject(store_);
}

LONG TsfProbe::LiveStores() noexcept {
  return InterlockedCompareExchange(&live_stores, 0, 0);
}

HRESULT TsfProbe::InitializeStoreForProbe(HWND hwnd,
                                        ContextIdentity identity) noexcept {
  if (thread_mgr_ || store_ || retired_) return E_UNEXPECTED;
  if (InterlockedIncrement(&live_stores) > kContextCapacity) {
    InterlockedDecrement(&live_stores);
    return HRESULT_FROM_WIN32(ERROR_NOT_ENOUGH_QUOTA);
  }
  store_ = new (std::nothrow) UnsupportedTextStore(hwnd, identity);
  if (!store_) {
    InterlockedDecrement(&live_stores);
    return E_OUTOFMEMORY;
  }
  return S_OK;
}

HRESULT TsfProbe::Initialize(HWND hwnd, ContextIdentity identity) noexcept {
  HRESULT hr = InitializeStoreForProbe(hwnd, identity);
  if (FAILED(hr)) return hr;
  hr = CoCreateInstance(CLSID_TF_ThreadMgr, nullptr,
                                CLSCTX_INPROC_SERVER,
                                IID_PPV_ARGS(&thread_mgr_));
  if (FAILED(hr)) return hr;
  ITfThreadMgrEx* extended = nullptr;
  hr = thread_mgr_->QueryInterface(IID_PPV_ARGS(&extended));
  if (FAILED(hr)) return hr;
  // 不激活／切换用户的键盘布局，不安装 profile 或 TIP。
  hr = extended->ActivateEx(&client_, TF_TMAE_NOACTIVATEKEYBOARDLAYOUT);
  extended->Release();
  if (FAILED(hr)) return hr;
  activated_ = true;
  hr = thread_mgr_->QueryInterface(IID_PPV_ARGS(&keystroke_mgr_));
  if (FAILED(hr)) return hr;
  hr = thread_mgr_->CreateDocumentMgr(&document_);
  if (FAILED(hr)) return hr;
  TfEditCookie cookie = 0;
  hr = document_->CreateContext(client_, 0, static_cast<ITextStoreACP*>(store_),
                                &context_, &cookie);
  if (FAILED(hr)) return hr;
  hr = document_->Push(context_);
  if (SUCCEEDED(hr)) pushed_ = true;
  return hr;
}

HRESULT TsfProbe::Focus() noexcept {
  if (retired_ || !document_) return CO_E_OBJNOTCONNECTED;
  return thread_mgr_->SetFocus(document_);
}

HRESULT TsfProbe::TestKey(const MSG& message, bool up, BOOL* eaten) noexcept {
  if (!eaten) return E_POINTER;
  *eaten = FALSE;
  if (retired_ || !keystroke_mgr_) return CO_E_OBJNOTCONNECTED;
  return up ? keystroke_mgr_->TestKeyUp(message.wParam, message.lParam, eaten)
            : keystroke_mgr_->TestKeyDown(message.wParam, message.lParam, eaten);
}

HRESULT TsfProbe::Key(const MSG& message, bool up, BOOL* eaten) noexcept {
  if (!eaten) return E_POINTER;
  *eaten = FALSE;
  if (retired_ || !keystroke_mgr_) return CO_E_OBJNOTCONNECTED;
  return up ? keystroke_mgr_->KeyUp(message.wParam, message.lParam, eaten)
            : keystroke_mgr_->KeyDown(message.wParam, message.lParam, eaten);
}

void TsfProbe::StopAdmissions() noexcept {
  if (store_) store_->Stop();
}

HRESULT TsfProbe::Retire() noexcept {
  if (retired_) return S_OK;
  StopAdmissions();
  if (thread_mgr_) {
    ITfDocumentMgr* focused = nullptr;
    HRESULT hr = thread_mgr_->GetFocus(&focused);
    if (FAILED(hr)) return hr;
    const bool own_focus = focused == document_;
    ReleaseObject(focused);
    if (own_focus) {
      hr = thread_mgr_->SetFocus(nullptr);
      if (FAILED(hr)) return hr;
    }
  }
  if (pushed_) {
    const HRESULT hr = document_->Pop(TF_POPF_ALL);
    if (FAILED(hr)) return hr;
    pushed_ = false;
  }
  ReleaseObject(context_);
  ReleaseObject(document_);
  ReleaseObject(keystroke_mgr_);
  if (activated_) {
    const HRESULT hr = thread_mgr_->Deactivate();
    if (FAILED(hr)) return hr;
    activated_ = false;
  }
  ReleaseObject(thread_mgr_);
  retired_ = true;
  return S_OK;
}

bool TsfProbe::CanRelease() const noexcept {
  return retired_ && (!store_ || store_->References() == 1);
}

HRESULT TsfProbe::AcquireStore(IUnknown** object) noexcept {
  if (!object) return E_POINTER;
  *object = nullptr;
  if (!store_) return CO_E_OBJNOTCONNECTED;
  return store_->QueryInterface(IID_IUnknown, reinterpret_cast<void**>(object));
}

void TsfProbe::ReleaseSystemObjects() noexcept {
  // 正常路径使用 Retire 并检查 HRESULT；析构仅为关闭本探针的兜底清理，
  // 不等于 core revoke 确认，更不授予下一 editable owner。
  if (pushed_ && document_) document_->Pop(TF_POPF_ALL);
  pushed_ = false;
  ReleaseObject(context_);
  ReleaseObject(document_);
  ReleaseObject(keystroke_mgr_);
  if (activated_ && thread_mgr_) thread_mgr_->Deactivate();
  activated_ = false;
  ReleaseObject(thread_mgr_);
}

}  // namespace guosshell::native_input_spike
