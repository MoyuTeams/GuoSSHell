#include "native_input_spike_sdk_channel.h"
#include "flutter_window.h"

#include <flutter/standard_method_codec.h>

#include <algorithm>
#include <array>
#include <cstdio>
#include <optional>
#include <string>
#include <vector>

namespace {
using flutter::EncodableMap;
using flutter::EncodableValue;
using guosshell::native_input_spike_sdk::Diagnostic;
using guosshell::native_input_spike_sdk::Status;
namespace live = guosshell::native_input_spike_live_source;
namespace source = guosshell::native_input_spike;
namespace abi = guosshell::native_input_spike_abi;

// 固定一个 retained handle；不引用已销毁的 channel、Flutter 或 Dart。
std::optional<live::SourceFixtureHost::RetainedOwner> retained_source;
UINT_PTR retirement_timer = 0;
bool retirement_disabled = false;
HRESULT retirement_status = S_OK;
live::Numbers retired_numbers{};

void CALLBACK RetirementTick(HWND, UINT, UINT_PTR, DWORD) {
  NativeInputSdkChannel::PollRetainedSource();
}

EncodableMap EncodeSource(const live::Numbers& n) {
  EncodableMap out;
  const auto number = [&out](const char* name, std::uint64_t value) {
    out.emplace(EncodableValue(name), EncodableValue(static_cast<int64_t>(value)));
  };
  out.emplace(EncodableValue("sourceActualSameSdk"), EncodableValue(n.caller == live::CallerEvidence::ParentVerifiedSdk));
  out.emplace(EncodableValue("syntheticOnly"), EncodableValue(true));
  out.emplace(EncodableValue("physical"), EncodableValue(false));
  number("origin", static_cast<std::uint32_t>(n.origin));
  number("status", n.status);
  number("observations", n.observation_count);
  number("syntheticPosts", n.synthetic_posts);
  number("accepting", n.accepting);
  number("coreRevoked", n.core_revoked);
  number("actualShutdownConfirmed", n.actual_shutdown_confirmed);
  number("managementStarted", n.management_started);
  number("managementCompleted", n.management_completed);
  number("joinStatus", n.join_status);
  number("owners", n.owners);
  number("chargedSlots", n.charged_slots);
  number("chargedBytes", n.charged_bytes);
  number("inflight", n.inflight);
  number("stopped", n.stopped);
  number("workerDone", n.worker_done);
  number("workerJoined", n.worker_joined);
  number("remoteApplied", n.remote_applied);
  number("nextSeq", n.next_seq);
  number("lastWrittenSeq", n.last_written_seq);
  number("lastEvent", n.last.event_id);
  number("lastPhase", n.last.phase);
  number("lastRawSource", n.last.raw_source);
  number("lastStatus", n.last.result.status);
  number("lastDisposition", n.last.result.disposition);
  number("lastSeq", n.last.result.seq);
  number("lastAction", n.last.result.action);
  number("lastByteLength", n.last.result.byte_len);
  number("gate", n.source_counters.gate);
  number("tsfTestDown", n.source_counters.tsf_test_down);
  number("tsfDown", n.source_counters.tsf_down);
  number("tsfTestUp", n.source_counters.tsf_test_up);
  number("tsfUp", n.source_counters.tsf_up);
  number("characterBlocked", n.source_counters.character_messages_blocked);
  number("pointerBlocked", n.source_counters.pointer_messages_blocked);
  number("missingPretranslate", n.source_counters.missing_pretranslate);
  number("textSupport", abi::kUnsupported);
  number("pasteSupport", abi::kUnsupported);
  number("pointerSupport", abi::kUnsupported);
  return out;
}

void EncodeClose(EncodableMap& map, const live::CloseResult& close) {
  map.emplace(EncodableValue("closeHr"), EncodableValue(static_cast<int32_t>(close.hr)));
  map.emplace(EncodableValue("closeComplete"), EncodableValue(close.hr == S_OK));
}

struct NamedProbe {
  const char* name;
  std::uint32_t function;
  source::KeyPhase phase;
};

// 只有固定非秘密探针；function=0 是来源未知的固定 VK_A 负向路径。
constexpr NamedProbe kProbes[] = {
    {"f1Down", 1, source::KeyPhase::Down}, {"f1Up", 1, source::KeyPhase::Up},
    {"f2Down", 2, source::KeyPhase::Down}, {"f2Repeat", 2, source::KeyPhase::Repeat},
    {"f2Up", 2, source::KeyPhase::Up}, {"f3Down", 3, source::KeyPhase::Down},
    {"f3Up", 3, source::KeyPhase::Up}, {"f4Down", 4, source::KeyPhase::Down},
    {"f4Repeat", 4, source::KeyPhase::Repeat}, {"f4Up", 4, source::KeyPhase::Up},
    {"f5Down", 5, source::KeyPhase::Down}, {"f5Up", 5, source::KeyPhase::Up},
    {"f6Down", 6, source::KeyPhase::Down}, {"f6Up", 6, source::KeyPhase::Up},
    {"unknownDown", 0, source::KeyPhase::Down},
    {"unknownRepeat", 0, source::KeyPhase::Repeat},
    {"unknownUp", 0, source::KeyPhase::Up}};

const EncodableValue* Find(const EncodableMap& map, const char* name) {
  const auto found = map.find(EncodableValue(name));
  return found == map.end() ? nullptr : &found->second;
}

EncodableMap Encode(const Diagnostic& diagnostic) {
  return {
      {EncodableValue("status"), EncodableValue(static_cast<int32_t>(diagnostic.status))},
      {EncodableValue("coreAbi"), EncodableValue(static_cast<int32_t>(diagnostic.core_abi))},
      {EncodableValue("pointerBits"), EncodableValue(static_cast<int32_t>(diagnostic.pointer_bits))},
      {EncodableValue("owners"), EncodableValue(static_cast<int32_t>(diagnostic.owners))},
      {EncodableValue("rinfExports"), EncodableValue(static_cast<int32_t>(diagnostic.rinf_exports))},
      {EncodableValue("coreExports"), EncodableValue(static_cast<int32_t>(diagnostic.core_exports))},
      {EncodableValue("fixtureOnly"), EncodableValue(diagnostic.fixture_only)},
      {EncodableValue("sameModule"), EncodableValue(diagnostic.same_module)},
  };
}

bool DecodePath(const std::string& utf8, std::wstring* path) {
  if (utf8.empty() || utf8.size() > 32768 || utf8.find('\0') != std::string::npos) {
    return false;
  }
  const int count = static_cast<int>(utf8.size());
  const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(), count, nullptr, 0);
  if (size <= 0) return false;
  path->resize(static_cast<std::size_t>(size));
  return MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, utf8.data(), count, path->data(), size) == size;
}
}  // namespace

NativeInputSdkChannel::NativeInputSdkChannel(flutter::BinaryMessenger* messenger,
                                           HWND parent, bool source_enabled,
                                           bool source_exit_test_enabled)
    : parent_(parent), source_enabled_(source_enabled),
      source_exit_test_enabled_(source_enabled && source_exit_test_enabled) {
  channel_ = std::make_unique<flutter::MethodChannel<EncodableValue>>(
      messenger, guosshell::native_input_spike_sdk::kChannel,
      &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler([this](const auto& call, auto result) {
    Handle(call, std::move(result));
  });
}

NativeInputSdkChannel::~NativeInputSdkChannel() {
  channel_->SetMethodCallHandler(nullptr);
  if (source_) {
    const auto close = source_->BeginClose();
    if (close.hr != S_OK) {
      // 同 STA 接回一次；Pending 不释放 Root，也不在 UI join worker。
      auto owner = source_->DetachForUiPoll();
      if (owner.pending()) {
        retained_source.emplace(std::move(owner));
        retirement_status = close.hr;
        retirement_timer = SetTimer(nullptr, 0, 16, RetirementTick);
        if (!retirement_timer) {
          std::fprintf(stderr, "[source-fixture] retained=1 timer=0 closeHr=%ld processExitPin=1\n", static_cast<long>(close.hr));
        }
      }
    }
    source_.reset();
  }
}

void NativeInputSdkChannel::PollRetainedSource() noexcept {
  if (retirement_disabled || !retained_source || !retained_source->pending()) return;
  const auto close = retained_source->PollClose();
  retirement_status = close.hr;
  if (close.hr == E_PENDING) return;
  const auto n = retained_source->InspectNumbers();
  retired_numbers = n;
  std::fprintf(stderr, "[source-fixture] closeHr=%ld management=%u join=%u owners=%u slots=%u bytes=%u inflight=%u stopped=%u done=%u joined=%u processExitPin=1\n",
      static_cast<long>(close.hr), n.management_completed, n.join_status, n.owners,
      n.charged_slots, n.charged_bytes, n.inflight, n.stopped, n.worker_done, n.worker_joined);
  if (retirement_timer) KillTimer(nullptr, retirement_timer);
  retirement_timer = 0;
  if (close.hr == S_OK) retained_source.reset();
  else retirement_disabled = true;
  // 失败仍保留 owner，但不反复调用已终结的管理工作。
}

HRESULT NativeInputSdkChannel::ReportRetainedAtExit() noexcept {
  // 消息循环已结束，不再依赖 UI timer，也不等待管理线程或泵入旧输入。
  if (retirement_timer) KillTimer(nullptr, retirement_timer);
  retirement_timer = 0;
  PollRetainedSource();
  if (!retained_source || !retained_source->pending()) return retirement_status;
  const auto n = retained_source->InspectNumbers();
  std::fprintf(stderr, "[source-fixture] retained=1 status=%u management=%u join=%u owners=%u slots=%u bytes=%u inflight=%u stopped=%u done=%u joined=%u processExitPin=1\n",
      n.status, n.management_completed, n.join_status, n.owners, n.charged_slots,
      n.charged_bytes, n.inflight, n.stopped, n.worker_done, n.worker_joined);
  return retirement_status;
}

HRESULT NativeInputSdkChannel::CloseForWindow() noexcept {
  if (exit_prepared_) return exit_retained_ ? retirement_status : exit_status_;
  return source_ ? source_->BeginClose().hr : S_OK;
}

live::Numbers NativeInputSdkChannel::InspectSourceNumbers() noexcept {
  if (source_) return source_->InspectNumbers();
  if (exit_retained_) {
    return retained_source && retained_source->pending()
        ? retained_source->InspectNumbers() : retired_numbers;
  }
  return exit_numbers_;
}

HRESULT NativeInputSdkChannel::PrepareForProcessExit() noexcept {
  if (exit_prepared_) return CloseForWindow();
  exit_prepared_ = true;
  if (!source_) return S_OK;
  exit_status_ = source_->BeginClose().hr;
  exit_numbers_ = source_->InspectNumbers();
  if (exit_status_ != S_OK) {
    auto owner = source_->DetachForUiPoll();
    if (owner.pending()) {
      // 唯一 Root 转移到固定槽；旧 channel 析构不再操作 COM 或启动 source。
      retained_source.emplace(std::move(owner));
      exit_retained_ = true;
      retirement_status = exit_status_;
    } else {
      exit_numbers_ = source_->InspectNumbers();
      exit_status_ = exit_numbers_.caller == live::CallerEvidence::None
          ? S_OK : source_->PollClose().hr;
    }
  }
  source_.reset();
  return CloseForWindow();
}

source::LoopDecision NativeInputSdkChannel::BeforeTranslate(const MSG& message) noexcept {
  if (!source_) return {};
  const auto decision = source_->BeforeTranslate(message);
  if (decision.route == source::LoopRoute::Handled) ++loop_handled_;
  else if (decision.route == source::LoopRoute::DispatchOnly) ++loop_dispatch_only_;
  if (decision.route != source::LoopRoute::ExistingRoute) loop_stop_ = static_cast<std::uint32_t>(decision.stop);
  return decision;
}

void NativeInputSdkChannel::Handle(
    const flutter::MethodCall<EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<EncodableValue>> result) {
  if (call.method_name().rfind("source", 0) == 0) {
    if (!source_enabled_) {
      result->Success(EncodableValue(EncodeSource({})));
      return;
    }
    const auto* args = std::get_if<EncodableMap>(call.arguments());
    auto numbers = [this]() {
      auto out = EncodeSource(InspectSourceNumbers());
      out.emplace(EncodableValue("exitPrepared"), EncodableValue(exit_prepared_));
      out.emplace(EncodableValue("loopHandled"), EncodableValue(static_cast<int64_t>(loop_handled_)));
      out.emplace(EncodableValue("loopDispatchOnly"), EncodableValue(static_cast<int64_t>(loop_dispatch_only_)));
      out.emplace(EncodableValue("loopStop"), EncodableValue(static_cast<int64_t>(loop_stop_)));
      return out;
    };
    if (call.method_name() == "sourceTestDirectExit") {
      if (!source_exit_test_enabled_ || !source_ || source_->InspectNumbers().accepting != 1) {
        result->Error("disabled", "固定退出测试未开启或 source 未存活");
        return;
      }
      // 只消费自己的固定 WM_QUIT；不 Translate／Dispatch 输入，不卸载 COM。
      PostQuitMessage(0);
      MSG quit{};
      const int retrieved = GetMessageW(&quit, nullptr, WM_QUIT, WM_QUIT);
      const bool quit_received = retrieved == 0 && quit.message == WM_QUIT;
      const HRESULT finish = quit_received ? FlutterWindow::FinishOptionalFixture() : E_FAIL;
      APTTYPE apartment{};
      APTTYPEQUALIFIER qualifier{};
      const HRESULT com = CoGetApartmentType(&apartment, &qualifier);
      auto out = numbers();
      out.emplace(EncodableValue("exitLoopQuit"), EncodableValue(quit_received));
      out.emplace(EncodableValue("finishHr"), EncodableValue(static_cast<int32_t>(finish)));
      out.emplace(EncodableValue("normalTeardownAllowed"), EncodableValue(finish == S_OK));
      out.emplace(EncodableValue("comStillSta"), EncodableValue(com == S_OK &&
          (apartment == APTTYPE_STA || apartment == APTTYPE_MAINSTA)));
      result->Success(EncodableValue(std::move(out)));
    } else if (call.method_name() == "sourceOpen") {
      const auto* opt_value = args ? Find(*args, "explicitOptIn") : nullptr;
      const auto* opt = opt_value ? std::get_if<bool>(opt_value) : nullptr;
      HRESULT hr = E_ACCESSDENIED;
      HRESULT focus = S_FALSE;
      if (opt && *opt && !exit_prepared_ && !source_ && host_.verified_core_module()) {
        source_ = std::make_unique<live::SourceFixtureHost>(live::FixtureOptions{true, true});
        hr = source_->OpenSource(parent_, GetModuleHandleW(nullptr), host_);
        if (hr == S_OK) focus = source_->FocusOwnFixture();
      }
      auto out = numbers();
      out.emplace(EncodableValue("openHr"), EncodableValue(static_cast<int32_t>(hr)));
      out.emplace(EncodableValue("focusHr"), EncodableValue(static_cast<int32_t>(focus)));
      result->Success(EncodableValue(std::move(out)));
    } else if (call.method_name() == "sourceInspect") {
      result->Success(EncodableValue(numbers()));
    } else if (call.method_name() == "sourceProbe") {
      const auto* name_value = args ? Find(*args, "probe") : nullptr;
      const auto* name = name_value ? std::get_if<std::string>(name_value) : nullptr;
      std::uint32_t status = exit_prepared_ ? abi::kStopped : source_ ? abi::kInvalid : abi::kDisabled;
      if (name && source_) {
        for (const auto& probe : kProbes) {
          if (*name == probe.name) {
            status = probe.function == 0
                ? source_->TestUnknownScalarProbe(probe.phase)
                : source_->TestNamedProbe(probe.function, probe.phase);
            break;
          }
        }
      }
      result->Success(EncodableValue(EncodableMap{{EncodableValue("status"), EncodableValue(static_cast<int32_t>(status))}}));
    } else if (call.method_name() == "sourceMode") {
      const auto* command_value = args ? Find(*args, "command") : nullptr;
      const auto* command = command_value ? std::get_if<std::string>(command_value) : nullptr;
      std::uint32_t status = exit_prepared_ ? abi::kStopped : source_ ? abi::kInvalid : abi::kDisabled;
      const char* commands[] = {"query", "mode1", "mode2", "mode3"};
      if (command && source_) {
        for (std::uint32_t mode = 0; mode < 4; ++mode) {
          if (*command == commands[mode]) { status = source_->PeerMode(mode); break; }
        }
      }
      result->Success(EncodableValue(EncodableMap{{EncodableValue("status"), EncodableValue(static_cast<int32_t>(status))}}));
    } else if (call.method_name() == "sourceRead") {
      const auto bytes = source_ ? source_->ReadPeerBytes() : live::PeerBytes{};
      result->Success(EncodableValue(EncodableMap{
          {EncodableValue("status"), EncodableValue(static_cast<int32_t>(bytes.status))},
          {EncodableValue("bytes"), EncodableValue(std::vector<std::uint8_t>(bytes.bytes.begin(), bytes.bytes.begin() + bytes.size))}}));
    } else if (call.method_name() == "sourceCloseStatus") {
      const auto* begin_value = args ? Find(*args, "begin") : nullptr;
      const auto* begin = begin_value ? std::get_if<bool>(begin_value) : nullptr;
      live::CloseResult close;
      if (exit_prepared_) {
        if (exit_retained_) PollRetainedSource();
        close.hr = CloseForWindow();
      } else if (source_) {
        close = begin && *begin ? source_->BeginClose() : source_->PollClose();
      }
      auto out = numbers();
      EncodeClose(out, close);
      result->Success(EncodableValue(std::move(out)));
    } else {
      result->NotImplemented();
    }
    return;
  }
  if (exit_prepared_) {
    result->Error("stopped", "夹具已停止，不再启动 SDK 或 source");
    return;
  }
  if (call.method_name() != "preflight" && call.method_name() != "verify") {
    result->NotImplemented();
    return;
  }
  Diagnostic diagnostic;
  diagnostic.status = Status::kInvalidReply;
  std::wstring canonical;
  try {
    const auto* args = std::get_if<EncodableMap>(call.arguments());
    if (args != nullptr && call.method_name() == "preflight") {
      diagnostic.status = Status::kInvalidPath;
      const auto* path_value = Find(*args, "path");
      const auto* opt_value = Find(*args, "explicitOptIn");
      const auto* nonce_value = Find(*args, "nonce");
      const auto* path = path_value == nullptr ? nullptr : std::get_if<std::string>(path_value);
      const auto* opt = opt_value == nullptr ? nullptr : std::get_if<bool>(opt_value);
      const auto* nonce = nonce_value == nullptr ? nullptr : std::get_if<std::vector<uint8_t>>(nonce_value);
      if (opt != nullptr && !*opt) {
        diagnostic.status = Status::kDisabled;
      } else if (path != nullptr && opt != nullptr && nonce != nullptr && nonce->size() == 16) {
        std::wstring decoded;
        if (DecodePath(*path, &decoded)) {
          std::array<uint8_t, 16> borrowed_nonce{};
          std::copy(nonce->begin(), nonce->end(), borrowed_nonce.begin());
          diagnostic = host_.Preflight(decoded, true, borrowed_nonce);
          if (diagnostic.status == Status::kOk) canonical = host_.compiled_lib_path();
        }
      }
    } else if (args != nullptr) {
      const auto* reply_value = Find(*args, "reply");
      const auto* reply = reply_value == nullptr ? nullptr : std::get_if<std::vector<uint8_t>>(reply_value);
      if (reply != nullptr && reply->size() == 48) {
        diagnostic = host_.Verify(reply->data(), reply->size());
      }
    }
  } catch (...) {
    diagnostic = Diagnostic{};
    diagnostic.status = Status::kLoadFailed;
    canonical.clear();
  }
  auto encoded = Encode(diagnostic);
  if (!canonical.empty()) {
    const int count = static_cast<int>(canonical.size());
    const int size = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, canonical.data(), count, nullptr, 0, nullptr, nullptr);
    if (size <= 0) {
      diagnostic = Diagnostic{};
      diagnostic.status = Status::kInvalidPath;
      encoded = Encode(diagnostic);
    } else {
      std::string utf8(static_cast<std::size_t>(size), '\0');
      if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, canonical.data(), count, utf8.data(), size, nullptr, nullptr) != size) {
        diagnostic = Diagnostic{};
        diagnostic.status = Status::kInvalidPath;
        encoded = Encode(diagnostic);
      } else {
        encoded.emplace(EncodableValue("compiledLibPath"), EncodableValue(std::move(utf8)));
      }
    }
  }
  result->Success(EncodableValue(std::move(encoded)));
}
