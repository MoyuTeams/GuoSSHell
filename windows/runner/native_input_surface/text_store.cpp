#include "text_store.h"
#include <olectl.h>

#include <algorithm>
#include <cstring>
#include <limits>

namespace guossh::native_input {
using Microsoft::WRL::ComPtr;

TextStore::TextStore(HWND hwnd, OwnerRef owner, std::shared_ptr<Callbacks> callbacks,
                     std::shared_ptr<NativePermit> permit) noexcept
    : thread_(GetCurrentThreadId()), hwnd_(hwnd), owner_(owner),
      callbacks_(std::move(callbacks)), permit_(std::move(permit)) {}
TextStore* TextStore::Create(HWND hwnd, OwnerRef owner, std::shared_ptr<Callbacks> callbacks,
                            std::shared_ptr<NativePermit> permit) noexcept {
  if (!permit) return nullptr;
  void* memory = permit->Memory().Allocate(sizeof(TextStore));
  if (!memory) return nullptr;
  return ::new (memory) TextStore(hwnd, owner, std::move(callbacks), std::move(permit));
}

bool TextStore::OnThread() const noexcept { return GetCurrentThreadId() == thread_; }
bool TextStore::Boundary(LONG p) const noexcept {
  return p >= 0 && p <= length_ &&
         !(p > 0 && p < length_ && HighSurrogate(text_[p - 1]) && LowSurrogate(text_[p]));
}
bool TextStore::Range(LONG a, LONG b) const noexcept { return a <= b && Boundary(a) && Boundary(b); }
bool TextStore::ReadLocked() const noexcept { return OnThread() && (lock_ & TS_LF_READ); }
bool TextStore::WriteLocked() const noexcept {
  return OnThread() && (lock_ & TS_LF_READWRITE) == TS_LF_READWRITE;
}
bool TextStore::Editable() const noexcept {
  return bound_ && !sealed_ && !retired_ &&
         !(notifying_ && scope_.route == TextRoute::TranslateStore) &&
         (in_dispatch_ || composition_ || end_composition_pending_);
}
HRESULT TextStore::Bind(const TicketRef& ticket) noexcept {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (retired_ || sealed_ || !permit_ || !callbacks_) return TS_E_READONLY;
  if (bound_) return ticket == ticket_ ? S_OK : E_ACCESSDENIED;
  if (!ticket.transaction || ticket.context != owner_.context ||
      ticket.owner_generation != owner_.generation) return E_ACCESSDENIED;
  ticket_ = ticket;
  bound_ = true;
  if (!callbacks_->ReservePayload(ticket_, kTextBytes, &grant_) ||
      grant_.bytes < kTextBytes || !grant_.chunk_bytes) {
    Cancel(Fault::PayloadRejected);
    return E_OUTOFMEMORY;
  }
  if (retired_ || sealed_) return E_ABORT;
  return S_OK;
}
HRESULT TextStore::BeginDispatch(const TextDispatchScope& scope) noexcept {
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (in_dispatch_ || lock_ || requesting_ || notifying_ || pending_lock_) {
    Cancel(Fault::AmbiguousTextRoute);
    return E_UNEXPECTED;
  }
  if (scope.context != owner_.context || !bound_ || !(scope.ticket == ticket_) || sealed_ || retired_)
    return E_ACCESSDENIED;
  scope_ = scope;
  in_dispatch_ = true;
  return S_OK;
}
HRESULT TextStore::EndDispatch() noexcept {
  ComPtr<TextStore> self(this);
  if (!OnThread() || !in_dispatch_) return E_UNEXPECTED;
  if (sealed_ || retired_) { in_dispatch_ = false; return failure_ == Fault::None ? S_FALSE : E_ABORT; }
  if (lock_ || requesting_) { in_dispatch_ = false; return TS_S_ASYNC; }
  // Translate 产生的 app edit 通知仍属于同一 dispatch。通知触发的只读 TSF
  // edit session 不是独立输入；必须收完这些回调后才结束 scope。
  HRESULT hr = PumpNotifications();
  in_dispatch_ = false;
  if (FAILED(hr)) return hr;
  if (composition_ || dead_pending_) return PublishPreedit();
  return Finalize();
}
bool TextStore::DispatchActive(const TextDispatchScope& scope) const noexcept {
  return OnThread() && !retired_ && !sealed_ && bound_ && in_dispatch_ &&
         scope.context == owner_.context && scope.context == scope_.context &&
         scope.event_id == scope_.event_id && scope.ticket == ticket_ && scope.ticket == scope_.ticket;
}
HRESULT TextStore::SelectTranslateRoute(const TextDispatchScope& scope) noexcept {
  if (!DispatchActive(scope) || composition_ || lock_ || pending_lock_ || scope.route != TextRoute::TranslateStore)
    return E_UNEXPECTED;
  scope_ = scope;
  return S_OK;
}
void TextStore::Cancel(Fault reason) noexcept {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return;
  if (bound_ && !sealed_) {
    sealed_ = true;
    failure_ = reason;
    callbacks_->Cancel(ticket_, reason);
  }
}
void TextStore::Retire() noexcept {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return;
  retired_ = true;
  Cancel(Fault::TextContextUnverified);
  pending_lock_ = 0;
}
bool TextStore::CanDetach() const noexcept {
  return OnThread() && !lock_ && !requesting_ && !notifying_ && !in_dispatch_ &&
         !pending_lock_ && !composition_ && !dead_pending_;
}
HRESULT TextStore::PublishPreedit() noexcept {
  if (sealed_ || retired_) return TS_E_READONLY;
  if (published_revision_ == revision_) return S_OK;
  published_revision_ = revision_;
  if (!callbacks_->Preedit(ticket_, revision_, text_.data(), length_,
                            composing_start_, composing_end_)) {
    Cancel(Fault::PayloadRejected);
    return E_FAIL;
  }
  return retired_ || sealed_ ? E_ABORT : S_OK;
}
HRESULT TextStore::Finalize() noexcept {
  if (failure_ != Fault::None) return E_ABORT;
  if (!bound_ || sealed_) return S_FALSE;
  if (retired_ || composition_ || end_composition_pending_ || dead_pending_ || in_dispatch_ || requesting_ ||
      notifying_ || lock_ || pending_lock_) return TS_S_ASYNC;
  if (!ValidUtf16(text_.data(), length_)) {
    Cancel(Fault::InvalidUtf16);
    return E_INVALIDARG;
  }
  const int bytes = length_ ? WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS,
      text_.data(), length_, utf8_.data(), static_cast<int>(utf8_.size()), nullptr, nullptr) : 0;
  if (length_ && !bytes) {
    Cancel(Fault::InvalidUtf16);
    return E_INVALIDARG;
  }
  // 先封口；回调重入与重复 OnEndEdit 不得再次提交。
  sealed_ = true;
  uint32_t offset = 0;
  do {
    uint32_t count = std::min(grant_.chunk_bytes, static_cast<uint32_t>(bytes) - offset);
    if (offset + count < static_cast<uint32_t>(bytes)) {
      while (count && (static_cast<unsigned char>(utf8_[offset + count]) & 0xc0) == 0x80) --count;
      if (!count) {
        failure_ = Fault::PayloadRejected;
        callbacks_->Cancel(ticket_, Fault::PayloadRejected);
        return E_FAIL;
      }
    }
    const bool last = offset + count == static_cast<uint32_t>(bytes);
    if (!callbacks_->Payload(ticket_, grant_, offset, utf8_.data() + offset, count, last)) {
      failure_ = Fault::PayloadRejected;
      callbacks_->Cancel(ticket_, Fault::PayloadRejected);
      return E_FAIL;
    }
    if (retired_) return E_ABORT;
    offset += count;
    if (last) break;
  } while (offset < static_cast<uint32_t>(bytes));
  if (!callbacks_->Finish(ticket_, revision_)) {
    failure_ = Fault::PayloadRejected;
    callbacks_->Cancel(ticket_, Fault::PayloadRejected);
    return E_FAIL;
  }
  return retired_ ? E_ABORT : S_OK;
}

HRESULT TextStore::QueryInterface(REFIID iid, void** out) {
  if (!out) return E_POINTER;
  *out = nullptr;
  if (iid == IID_IUnknown || iid == IID_ITextStoreACP) *out = static_cast<ITextStoreACP*>(this);
  else if (iid == IID_ITfContextOwnerCompositionSink) *out = static_cast<ITfContextOwnerCompositionSink*>(this);
  else if (iid == IID_ITfTextEditSink) *out = static_cast<ITfTextEditSink*>(this);
  else return E_NOINTERFACE;
  AddRef();
  return S_OK;
}
ULONG TextStore::AddRef() { return ++references_; }
ULONG TextStore::Release() {
  const ULONG n = --references_;
  if (!n) {
    auto permit = std::move(permit_);
    this->~TextStore();
    permit->Memory().Deallocate(this, sizeof(TextStore));
  }
  return n;
}
HRESULT TextStore::AdviseSink(REFIID iid, IUnknown* unknown, DWORD mask) {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (iid != IID_ITextStoreACPSink) return E_INVALIDARG;
  if (!unknown) return E_POINTER;
  ComPtr<IUnknown> identity;
  HRESULT hr = unknown->QueryInterface(IID_PPV_ARGS(&identity));
  if (FAILED(hr)) return hr;
  if (sink_identity_ && identity.Get() != sink_identity_.Get()) return CONNECT_E_ADVISELIMIT;
  ComPtr<ITextStoreACPSink> sink;
  hr = unknown->QueryInterface(IID_PPV_ARGS(&sink));
  if (FAILED(hr)) return hr;
  sink_ = sink;
  sink_identity_ = identity;
  sink_mask_ = mask;
  return S_OK;
}
HRESULT TextStore::UnadviseSink(IUnknown* unknown) {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (!unknown) return E_POINTER;
  ComPtr<IUnknown> identity;
  HRESULT hr = unknown->QueryInterface(IID_PPV_ARGS(&identity));
  if (FAILED(hr)) return hr;
  if (!sink_identity_ || identity.Get() != sink_identity_.Get()) return CONNECT_E_NOCONNECTION;
  sink_.Reset();
  sink_identity_.Reset();
  sink_mask_ = pending_lock_ = 0;
  return S_OK;
}
HRESULT TextStore::GrantLock(DWORD flags) noexcept {
  auto sink = sink_;
  if (!sink) return E_UNEXPECTED;
  lock_ = flags & TS_LF_READWRITE;
  HRESULT hr = sink->OnLockGranted(flags);
  lock_ = 0;
  return hr;
}
HRESULT TextStore::RequestLock(DWORD flags, HRESULT* result) {
  ComPtr<TextStore> self(this);
  if (!result) return E_POINTER;
  *result = E_FAIL;
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (!sink_) return E_UNEXPECTED;
  if (!(flags & TS_LF_READ) || (flags & ~(TS_LF_SYNC | TS_LF_READWRITE))) return E_INVALIDARG;
  if (lock_ || requesting_ || notifying_ || pending_text_) {
    if (flags & TS_LF_SYNC) *result = TS_E_SYNCHRONOUS;
    else {
      pending_lock_ |= flags & TS_LF_READWRITE;
      *result = TS_S_ASYNC;
    }
    return S_OK;
  }
  requesting_ = true;
  // 只有本次成功授予覆盖了旧请求的权限，才可结算 fixed pending 槽。
  if ((flags & pending_lock_) == pending_lock_) pending_lock_ = 0;
  *result = GrantLock(flags);
  // 必需的 read→write 升级仅有一个槽；不执行无限重入队列。
  if (pending_lock_) {
    const DWORD upgrade = pending_lock_;
    pending_lock_ = 0;
    GrantLock(upgrade);
  }
  requesting_ = false;
  return S_OK;
}
HRESULT TextStore::GetStatus(TS_STATUS* status) {
  if (!status) return E_POINTER;
  status->dwDynamicFlags = retired_ || sealed_ ? TS_SD_READONLY : 0;
  status->dwStaticFlags = TS_SS_NOHIDDENTEXT;
  return S_OK;
}
HRESULT TextStore::QueryInsert(LONG a, LONG b, ULONG n, LONG* start, LONG* end) {
  if (!start || !end) return E_POINTER;
  if (!Range(a, b)) return TS_E_INVALIDPOS;
  if (n > kTextUnits - static_cast<ULONG>(length_ - (b - a))) return E_OUTOFMEMORY;
  *start = a;
  *end = b;
  return S_OK;
}
HRESULT TextStore::GetSelection(ULONG index, ULONG count, TS_SELECTION_ACP* out, ULONG* fetched) {
  if (!fetched || (count && !out)) return E_POINTER;
  *fetched = 0;
  if (!ReadLocked()) return TS_E_NOLOCK;
  if (index != 0 && index != TS_DEFAULT_SELECTION) return TS_E_NOSELECTION;
  if (count) { *out = selection_; *fetched = 1; }
  return S_OK;
}
HRESULT TextStore::SetSelection(ULONG count, const TS_SELECTION_ACP* value) {
  ComPtr<TextStore> self(this);
  if (!WriteLocked()) return TS_E_NOLOCK;
  if (scope_.route == TextRoute::TranslateStore && !sealed_) {
    Cancel(Fault::AmbiguousTextRoute);
    return E_UNEXPECTED;
  }
  if (!Editable()) return TS_E_READONLY;
  if (count != 1 || !value) return E_INVALIDARG;
  if (!Range(value->acpStart, value->acpEnd)) return TS_E_INVALIDPOS;
  if (value->style.ase != TS_AE_START && value->style.ase != TS_AE_END && value->style.ase != TS_AE_NONE)
    return E_INVALIDARG;
  if (revision_ == UINT64_MAX) { Cancel(Fault::TextCapacity); return E_FAIL; }
  selection_ = *value;
  ++revision_;
  ++side_effects_;
  return S_OK;
}
HRESULT TextStore::GetText(LONG a, LONG b, WCHAR* plain, ULONG plain_cap, ULONG* plain_n,
                           TS_RUNINFO* runs, ULONG run_cap, ULONG* run_n, LONG* next) {
  if (!plain_n || !run_n || !next || (plain_cap && !plain) || (run_cap && !runs)) return E_POINTER;
  *plain_n = *run_n = 0;
  if (!ReadLocked()) return TS_E_NOLOCK;
  if (b == -1) b = length_;
  if (!Range(a, b)) return TS_E_INVALIDPOS;
  *next = a;
  LONG n = b - a;
  if (plain_cap) n = std::min(n, static_cast<LONG>(std::min<ULONG>(plain_cap, kTextUnits)));
  else if (!run_cap) n = 0;
  if (!Boundary(a + n)) --n;
  if (!n && a < b && plain_cap) return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
  if (plain_cap && n) {
    std::memcpy(plain, text_.data() + a, n * sizeof(WCHAR));
    *plain_n = n;
  }
  if (run_cap && n) { runs[0] = {static_cast<ULONG>(n), TS_RT_PLAIN}; *run_n = 1; }
  *next = a + n;
  return S_OK;
}
HRESULT TextStore::Replace(LONG a, LONG b, const WCHAR* value, ULONG count, TS_TEXTCHANGE* change) noexcept {
  ComPtr<TextStore> self(this);
  if (!change || (!value && count)) return E_POINTER;
  if (scope_.route == TextRoute::TranslateStore && (lock_ || requesting_ || notifying_) && !sealed_) {
    Cancel(Fault::AmbiguousTextRoute);
    return E_UNEXPECTED;
  }
  if (!Editable()) return TS_E_READONLY;
  if (!Range(a, b) || a < frozen_prefix_) return TS_E_INVALIDPOS;
  if (count > kTextUnits - static_cast<ULONG>(length_ - (b - a))) {
    Cancel(Fault::TextCapacity);
    return E_OUTOFMEMORY;
  }
#ifdef GUOSSH_NATIVE_FIXTURE
  if (!ValidUtf16(value, count, examined_)) return E_INVALIDARG;
#else
  if (!ValidUtf16(value, count)) return E_INVALIDARG;
#endif
  if (revision_ == UINT64_MAX) { Cancel(Fault::TextCapacity); return E_FAIL; }
  std::memmove(text_.data() + a + count, text_.data() + b, (length_ - b) * sizeof(WCHAR));
  if (count) std::memcpy(text_.data() + a, value, count * sizeof(WCHAR));
  *change = {a, b, a + static_cast<LONG>(count)};
  length_ += static_cast<LONG>(count) - (b - a);
  selection_ = {change->acpNewEnd, change->acpNewEnd, {TS_AE_END, FALSE}};
  ++revision_;
  ++side_effects_;
  layout_count_ = 0;
  return S_OK;
}
HRESULT TextStore::SetText(DWORD flags, LONG a, LONG b, const WCHAR* text, ULONG n, TS_TEXTCHANGE* change) {
  ComPtr<TextStore> self(this);
  if (!WriteLocked()) return TS_E_NOLOCK;
  if (flags & ~TS_ST_CORRECTION) return E_INVALIDARG;
  return Replace(a, b, text, n, change);
}
HRESULT TextStore::InsertTextAtSelection(DWORD flags, const WCHAR* text, ULONG n,
                                         LONG* start, LONG* end, TS_TEXTCHANGE* change) {
  ComPtr<TextStore> self(this);
  if (!ReadLocked()) return TS_E_NOLOCK;
  if (flags & ~(TS_IAS_QUERYONLY | TS_IAS_NOQUERY)) return E_INVALIDARG;
  if ((flags & TS_IAS_QUERYONLY) && (flags & TS_IAS_NOQUERY)) return E_INVALIDARG;
  if (!(flags & TS_IAS_NOQUERY) && (!start || !end)) return E_POINTER;
  LONG a{}, b{};
  HRESULT hr = QueryInsert(selection_.acpStart, selection_.acpEnd, n, &a, &b);
  if (FAILED(hr)) return hr;
  if (!(flags & TS_IAS_NOQUERY)) { *start = a; *end = a + static_cast<LONG>(n); }
  if (flags & TS_IAS_QUERYONLY) return S_OK;
  if (!WriteLocked()) return TS_E_NOLOCK;
  return Replace(selection_.acpStart, selection_.acpEnd, text, n, change);
}
HRESULT TextStore::AppendTranslated(const WCHAR* value, ULONG count) noexcept {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (!in_dispatch_ || lock_ || requesting_ || notifying_ || composition_ ||
      scope_.route != TextRoute::TranslateStore) return E_UNEXPECTED;
  TS_TEXTCHANGE change{};
  HRESULT hr = Replace(length_, length_, value, count, &change);
  if (FAILED(hr)) return hr;
  if (!pending_text_) app_change_ = change;
  else app_change_.acpNewEnd = change.acpNewEnd;
  pending_text_ = pending_selection_ = true;
  return S_OK;
}
HRESULT TextStore::PumpNotifications() noexcept {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (lock_ || requesting_ || notifying_) return TS_S_ASYNC;
  auto sink = sink_;
  if (sink) {
    notifying_ = true;
    HRESULT hr = S_OK;
    if (pending_text_) {
      pending_text_ = false;
      if (sink_mask_ & TS_AS_TEXT_CHANGE) hr = sink->OnTextChange(0, &app_change_);
    }
    if (pending_selection_) {
      pending_selection_ = false;
      if (SUCCEEDED(hr) && (sink_mask_ & TS_AS_SEL_CHANGE)) hr = sink->OnSelectionChange();
    }
    if (pending_layout_) {
      pending_layout_ = false;
      if (SUCCEEDED(hr) && (sink_mask_ & TS_AS_LAYOUT_CHANGE)) hr = sink->OnLayoutChange(TS_LC_CHANGE, 1);
    }
    notifying_ = false;
    if (FAILED(hr)) { Cancel(Fault::AmbiguousTextRoute); return hr; }
  } else pending_text_ = pending_selection_ = pending_layout_ = false;
  if (pending_lock_) {
    const DWORD flags = pending_lock_;
    pending_lock_ = 0;
    requesting_ = true;
    HRESULT hr = GrantLock(flags);
    requesting_ = false;
    if (FAILED(hr)) return hr;
  }
  if (!in_dispatch_ && bound_ && !sealed_) {
    if (composition_ || dead_pending_) return PublishPreedit();
    return Finalize();
  }
  return S_OK;
}

HRESULT TextStore::GetFormattedText(LONG, LONG, IDataObject** out) {
  if (!out) return E_POINTER;
  *out = nullptr;
  return E_NOTIMPL;
}
HRESULT TextStore::GetEmbedded(LONG, REFGUID, REFIID, IUnknown** out) {
  if (!out) return E_POINTER;
  *out = nullptr;
  return TS_E_NOOBJECT;
}
HRESULT TextStore::QueryInsertEmbedded(const GUID*, const FORMATETC*, BOOL* value) {
  if (!value) return E_POINTER;
  *value = FALSE;
  return S_OK;
}
HRESULT TextStore::InsertEmbedded(DWORD, LONG, LONG, IDataObject*, TS_TEXTCHANGE*) { return TS_E_FORMAT; }
HRESULT TextStore::InsertEmbeddedAtSelection(DWORD, IDataObject*, LONG*, LONG*, TS_TEXTCHANGE*) { return TS_E_FORMAT; }
HRESULT TextStore::RequestSupportedAttrs(DWORD, ULONG count, const TS_ATTRID* attrs) {
  return count && !attrs ? E_POINTER : S_OK;
}
HRESULT TextStore::RequestAttrsAtPosition(LONG pos, ULONG count, const TS_ATTRID* attrs, DWORD flags) {
  if (!ReadLocked()) return TS_E_NOLOCK;
  if (!Boundary(pos)) return TS_E_INVALIDPOS;
  return RequestSupportedAttrs(flags, count, attrs);
}
HRESULT TextStore::RequestAttrsTransitioningAtPosition(LONG pos, ULONG n, const TS_ATTRID* attrs, DWORD flags) {
  return RequestAttrsAtPosition(pos, n, attrs, flags);
}
HRESULT TextStore::FindNextAttrTransition(LONG start, LONG halt, ULONG n, const TS_ATTRID* attrs,
                                         DWORD, LONG* next, BOOL* found, LONG* offset) {
  if (!next || !found || !offset || (n && !attrs)) return E_POINTER;
  if (!ReadLocked()) return TS_E_NOLOCK;
  if (!Boundary(start) || !Boundary(halt)) return TS_E_INVALIDPOS;
  *next = halt; *found = FALSE; *offset = 0;
  return S_OK;
}
HRESULT TextStore::RetrieveRequestedAttrs(ULONG n, TS_ATTRVAL* values, ULONG* fetched) {
  if (!fetched || (n && !values)) return E_POINTER;
  *fetched = 0;
  return S_OK;
}
HRESULT TextStore::GetEndACP(LONG* end) {
  if (!end) return E_POINTER;
  if (!ReadLocked()) return TS_E_NOLOCK;
  *end = length_;
  return S_OK;
}
HRESULT TextStore::GetActiveView(TsViewCookie* view) {
  if (!view) return E_POINTER;
  *view = 1;
  return S_OK;
}
void TextStore::SetGeometry(const SourceGeometry& geometry) noexcept {
  if (!OnThread()) return;
  geometry_ = geometry;
  if (geometry.revision.source != owner_.source) geometry_.acknowledged = false;
  layout_count_ = 0;
  pending_layout_ = true;
}
HRESULT TextStore::SetTextLayout(const RECT* rects, ULONG count, uint64_t revision) noexcept {
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (!geometry_.acknowledged || revision != revision_ || count != static_cast<ULONG>(length_ + 1))
    return TS_E_NOLAYOUT;
  if (!rects) return E_POINTER;
  for (ULONG i = 0; i < count; ++i)
    if (rects[i].left > rects[i].right || rects[i].top > rects[i].bottom) return E_INVALIDARG;
  std::copy_n(rects, count, acp_rects_.begin());
  layout_count_ = count;
  layout_revision_ = revision;
  pending_layout_ = true;
  return S_OK;
}
HRESULT TextStore::GetTextExt(TsViewCookie view, LONG a, LONG b, RECT* rect, BOOL* clipped) {
  if (!rect || !clipped) return E_POINTER;
  if (!ReadLocked()) return TS_E_NOLOCK;
  if (view != 1) return E_INVALIDARG;
  if (!Range(a, b)) return TS_E_INVALIDPOS;
  if (!geometry_.acknowledged || retired_) return TS_E_NOLAYOUT;
  RECT extent{};
  if (!length_) extent = geometry_.cursor_screen;
  else {
    if (layout_revision_ != revision_ || layout_count_ != static_cast<ULONG>(length_ + 1)) return TS_E_NOLAYOUT;
    extent = acp_rects_[a];
    for (LONG p = a + 1; p < b; ++p) UnionRect(&extent, &extent, &acp_rects_[p]);
  }
  if (!IntersectRect(rect, &extent, &geometry_.screen_clip)) SetRectEmpty(rect);
  *clipped = !EqualRect(rect, &extent);
  return S_OK;
}
HRESULT TextStore::GetACPFromPoint(TsViewCookie view, const POINT* point, DWORD flags, LONG* acp) {
  if (!point || !acp) return E_POINTER;
  if (!ReadLocked()) return TS_E_NOLOCK;
  if (view != 1 || (flags & ~(GXFPF_ROUND_NEAREST | GXFPF_NEAREST))) return E_INVALIDARG;
  if (!geometry_.acknowledged || retired_ ||
      (length_ && (layout_revision_ != revision_ || layout_count_ != static_cast<ULONG>(length_ + 1))))
    return TS_E_NOLAYOUT;
  int64_t best = INT64_MAX;
  LONG nearest = 0;
  for (LONG p = 0; p <= length_; ++p) {
    if (!Boundary(p)) continue;
    const RECT& r = length_ ? acp_rects_[p] : geometry_.cursor_screen;
    if (PtInRect(&r, *point)) { *acp = p; return S_OK; }
    const int64_t dx = static_cast<int64_t>(point->x) - std::clamp(point->x, r.left, r.right);
    const int64_t dy = static_cast<int64_t>(point->y) - std::clamp(point->y, r.top, r.bottom);
    const int64_t distance = std::abs(dx) + std::abs(dy);
    if (distance < best) { best = distance; nearest = p; }
  }
  if (!(flags & GXFPF_NEAREST)) return TS_E_INVALIDPOINT;
  *acp = nearest;
  return S_OK;
}
HRESULT TextStore::GetScreenExt(TsViewCookie view, RECT* rect) {
  if (!rect) return E_POINTER;
  if (view != 1) return E_INVALIDARG;
  if (!geometry_.acknowledged || retired_) return TS_E_NOLAYOUT;
  *rect = geometry_.screen_clip;
  return S_OK;
}
HRESULT TextStore::GetWnd(TsViewCookie view, HWND* hwnd) {
  if (!hwnd) return E_POINTER;
  if (view != 1) return E_INVALIDARG;
  *hwnd = hwnd_;
  return S_OK;
}
HRESULT TextStore::ReadCompositionRange(ITfRange* range) noexcept {
  if (!range) return E_POINTER;
  ComPtr<ITfRangeACP> acp;
  HRESULT hr = range->QueryInterface(IID_PPV_ARGS(&acp));
  if (FAILED(hr)) return hr;
  LONG start{}, count{};
  hr = acp->GetExtent(&start, &count);
  if (FAILED(hr)) return hr;
  if (count < 0 || start < frozen_prefix_ || start > length_ || count > length_ - start ||
      !Range(start, start + count)) return TS_E_INVALIDPOS;
  if (composing_start_ != start || composing_end_ != start + count) {
    if (revision_ == UINT64_MAX) { Cancel(Fault::TextCapacity); return E_FAIL; }
    ++revision_;
  }
  composing_start_ = start;
  composing_end_ = start + count;
  frozen_prefix_ = start;
  return S_OK;
}
HRESULT TextStore::OnStartComposition(ITfCompositionView* view, BOOL* accept) {
  ComPtr<TextStore> self(this);
  if (!accept) return E_POINTER;
  *accept = FALSE;
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (scope_.route == TextRoute::TranslateStore && !sealed_) {
    Cancel(Fault::AmbiguousTextRoute);
    return S_OK;
  }
  if (!view || !Editable() || composition_) return S_OK;
  ComPtr<ITfRange> range;
  HRESULT hr = view->GetRange(&range);
  if (FAILED(hr)) return hr;
  if (!Editable()) return S_OK;
  hr = ReadCompositionRange(range.Get());
  if (FAILED(hr)) return hr;
  composition_ = view;
  end_composition_pending_ = false;
  ++side_effects_;
  *accept = TRUE;
  return S_OK;
}
HRESULT TextStore::OnUpdateComposition(ITfCompositionView* view, ITfRange* range) {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (retired_ || sealed_) return S_OK;
  if (!composition_ || view != composition_.Get()) return E_UNEXPECTED;
  ++side_effects_;
  if (!range) return S_OK;
  HRESULT hr = ReadCompositionRange(range);
  if (FAILED(hr)) Cancel(Fault::AmbiguousTextRoute);
  return hr;
}
HRESULT TextStore::OnEndComposition(ITfCompositionView* view) {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (!composition_) return S_OK;
  if (view != composition_.Get()) return E_UNEXPECTED;
  composition_.Reset();
  end_composition_pending_ = true;
  ++side_effects_;
  return S_OK;
}
HRESULT TextStore::OnEndEdit(ITfContext* context, TfEditCookie cookie, ITfEditRecord* record) {
  ComPtr<TextStore> self(this);
  if (!OnThread()) return RPC_E_WRONG_THREAD;
  if (sealed_ || retired_ || !bound_) return S_OK;
  if (!context || !record) return E_POINTER;
  if (!in_dispatch_ && !composition_ && !end_composition_pending_) {
    Cancel(Fault::TextContextUnverified);
    return E_ACCESSDENIED;
  }
  // 从真实 TSF composition 枚举读取 ACP，不以 keydown 或文本内容猜提交。
  ComPtr<ITfContextComposition> compositions;
  HRESULT hr = context->QueryInterface(IID_PPV_ARGS(&compositions));
  if (FAILED(hr)) return hr;
  ComPtr<IEnumITfCompositionView> enumeration;
  hr = compositions->EnumCompositions(&enumeration);
  if (FAILED(hr)) return hr;
  ComPtr<ITfCompositionView> actual;
  ULONG fetched{};
  hr = enumeration->Next(1, &actual, &fetched);
  if (FAILED(hr)) return hr;
  if (fetched) {
    ComPtr<ITfRange> range;
    hr = actual->GetRange(&range);
    if (SUCCEEDED(hr)) hr = ReadCompositionRange(range.Get());
    if (FAILED(hr)) { Cancel(Fault::AmbiguousTextRoute); return hr; }
    ComPtr<ITfCompositionView> extra;
    hr = enumeration->Next(1, &extra, &fetched);
    if (FAILED(hr) || fetched) { Cancel(Fault::AmbiguousTextRoute); return E_UNEXPECTED; }
    if (!composition_) { Cancel(Fault::TextContextUnverified); return E_UNEXPECTED; }
  } else if (composition_) {
    Cancel(Fault::AmbiguousTextRoute);
    return E_UNEXPECTED;
  }
  BOOL selection_changed{};
  hr = record->GetSelectionStatus(&selection_changed);
  if (FAILED(hr)) return hr;
  (void)cookie;
  end_composition_pending_ = false;
  // 此处仍可能在 ACP lock 内；只发布有界 preedit，finalizer 由解锁后的 pump 调用。
  if (composition_) return PublishPreedit();
  return S_OK;
}

}  // namespace guossh::native_input
