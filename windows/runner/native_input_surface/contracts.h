#pragma once

#include <windows.h>
#include <msctf.h>
#include <array>
#include <cstdint>
#include <memory>
#include <new>
#include <limits>

namespace guossh::native_input {

// 仅为应用内 C++ 责任接口，不是共享 FFI ABI，也不负责绑定匹配或编码。
struct OwnerRef {
  uint64_t handle{}, source{}, session{}, generation{}, identity{}, context{};
};
struct TicketRef {
  uint64_t owner_generation{}, context{}, transaction{}, effect{};
  bool operator==(const TicketRef& b) const noexcept {
    return owner_generation == b.owner_generation && context == b.context &&
           transaction == b.transaction && effect == b.effect;
  }
};
enum class Fault : uint32_t {
  None, Busy, InvalidIdentity, TextCapacity, InvalidUtf16, PayloadRejected,
  AmbiguousTextRoute, TextContextUnverified, DeadStateRetirementUnproven,
  LayoutStateNotIsolated, LayoutUnverified, LayoutStale,
  AltGrUnverified, NativeRetirementPending, ContextBudget,
};
enum class FocusState { Active, CoreRevoking, NativeRetiring, Retired, NewEditable };
enum class NativeOrigin { DeclaredSynthetic, VerifiedNative };
enum class TextRoute { None, TsfStore, TranslateStore };
enum class Disposition { ConsumedDeferred, TerminalDeferred, Rejected, AllowNativeText };
enum class MetadataKind { Unknown, NoCharacter, Scalar, Dead, MultipleScalars, Named };
struct ScalarMetadata {
  MetadataKind kind{MetadataKind::Unknown};
  uint32_t scalar{};
};
struct ProfileIdentity {
  HKL hkl{};
  GUID clsid{}, profile{};
  LANGID language{};
  DWORD type{};
  uint64_t epoch{};
};
inline bool SameProfile(const ProfileIdentity& a, const ProfileIdentity& b) noexcept {
  return a.hkl == b.hkl && a.epoch == b.epoch && a.language == b.language &&
         a.type == b.type && IsEqualGUID(a.clsid, b.clsid) &&
         IsEqualGUID(a.profile, b.profile);
}
struct NativeKey {
  MSG message{};
  uint64_t event_id{};
  ProfileIdentity layout{};
  std::array<BYTE, 256> queue_state{};
  uint16_t scan{};
  bool extended{}, repeat{}, up{};
  ScalarMetadata raw{}, legacy{};
  Fault metadata_fault{Fault::LayoutUnverified};
  bool trusted_raw{};
  // canonical_raw 仅由 normal HRAWINPUT 适配器赋值；metadata 另行校验。
  bool canonical_raw{}, e1{};
  uint64_t physical{}, source_epoch{};
  std::array<BYTE, 256> physical_state{};
};
struct GateReceipt {
  Disposition disposition{Disposition::Rejected};
  TicketRef ticket{};
  bool trusted_raw{};
  // 已闭合票据的 Up 只平衡生命周期，不建立新事务。
  bool lifecycle_only{};
};
struct TextDispatchScope {
  TicketRef ticket{};
  uint64_t context{}, event_id{};
  TextRoute route{TextRoute::None};
};
struct GeometryRef {
  uint64_t source{}, frame{}, viewport{}, font{}, transform{};
};
struct SourceGeometry {
  GeometryRef revision{};
  RECT screen_clip{}, cursor_screen{};
  POINT client_origin_screen{};
  LONG cell_width{}, cell_height{}, rows{}, columns{};
  bool acknowledged{};
};
struct PointerEvent {
  UINT message{};
  POINT client{};
  uint32_t buttons{}, modifiers{};
  int wheel_delta{};
  GeometryRef geometry{};
  LONG row{}, column{};
};
struct PayloadGrant { uint64_t id{}; uint32_t bytes{}, chunk_bytes{}; };

// 父持有的租约必须在本对象及 core queued/inflight 各自引用都释放后才归还预算。
class NativeMemory {
 public:
  virtual ~NativeMemory() = default;
  virtual void* Allocate(size_t bytes) noexcept { return ::operator new(bytes, std::nothrow); }
  virtual void Deallocate(void* pointer, size_t) noexcept { ::operator delete(pointer); }
};
struct NativePermit {
  virtual ~NativePermit() = default;
  // allocator 的寿命属于授额 root；permit 必须覆盖对象及最后 deallocation 返回。
  virtual NativeMemory& Memory() noexcept = 0;
};
template<class T> struct PermitAllocator {
  using value_type = T;
  std::shared_ptr<NativePermit> permit;
  explicit PermitAllocator(std::shared_ptr<NativePermit> value) noexcept : permit(std::move(value)) {}
  template<class U> PermitAllocator(const PermitAllocator<U>& other) noexcept : permit(other.permit) {}
  T* allocate(size_t count) {
    if (count > (std::numeric_limits<size_t>::max)() / sizeof(T)) throw std::bad_alloc();
    void* memory = permit->Memory().Allocate(count * sizeof(T));
    if (!memory) throw std::bad_alloc();
    return static_cast<T*>(memory);
  }
  void deallocate(T* memory, size_t count) noexcept { permit->Memory().Deallocate(memory, count * sizeof(T)); }
  template<class U> bool operator==(const PermitAllocator<U>& other) const noexcept { return permit == other.permit; }
  template<class U> bool operator!=(const PermitAllocator<U>& other) const noexcept { return !(*this == other); }
};
class Callbacks {
 public:
  virtual ~Callbacks() = default;
  // 全部同步回调必须 nonblocking；调用前没有持有 core 锁。
  virtual GateReceipt Gate(const OwnerRef&, const NativeKey&,
                           const TicketRef* original) noexcept = 0;
  virtual bool ReservePayload(const TicketRef&, uint32_t bound,
                               PayloadGrant*) noexcept = 0;
  virtual bool PreparePayload(const TicketRef&, uint32_t) noexcept { return true; }
  virtual bool Preedit(const TicketRef&, uint64_t revision, const WCHAR*, ULONG,
                       LONG composing_start, LONG composing_end) noexcept = 0;
  virtual bool Payload(const TicketRef&, const PayloadGrant&, uint32_t offset,
                       const char*, uint32_t bytes, bool final) noexcept = 0;
  virtual bool Finish(const TicketRef&, uint64_t revision) noexcept = 0;
  virtual void Cancel(const TicketRef&, Fault) noexcept = 0;
  virtual void Faulted(const OwnerRef&, Fault) noexcept = 0;
  virtual bool CoreRevoke(const OwnerRef&, uint64_t* fence) noexcept = 0;
  virtual bool ConfirmRetired(const OwnerRef&, uint64_t fence) noexcept = 0;
  // context-local 结束不撤权 surface；回执只结算原事务，不等待 GUI claim。
  virtual bool ContextQuiet(const OwnerRef&, const TicketRef*, uint64_t) noexcept { return false; }
  virtual void Pointer(const OwnerRef&, const PointerEvent&) noexcept = 0;
};

constexpr ULONG kTextUnits = 4096;
constexpr uint32_t kTextBytes = kTextUnits * 3;
constexpr size_t kContextSlots = 8;
constexpr UINT kDrainLimit = kTextUnits;

inline bool HighSurrogate(WCHAR c) noexcept { return c >= 0xd800 && c <= 0xdbff; }
inline bool LowSurrogate(WCHAR c) noexcept { return c >= 0xdc00 && c <= 0xdfff; }
inline bool ValidUtf16(const WCHAR* text, ULONG count, ULONG* examined = nullptr) noexcept {
  if (!text && count) return false;
  for (ULONG i = 0; i < count; ++i) {
    if (examined) ++*examined;
    if (HighSurrogate(text[i])) {
      if (++i == count) return false;
      if (examined) ++*examined;
      if (!LowSurrogate(text[i])) return false;
    } else if (LowSurrogate(text[i])) return false;
  }
  return true;
}

}  // namespace guossh::native_input
