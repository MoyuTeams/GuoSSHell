#include "channel.h"

#include <flutter/standard_method_codec.h>

namespace guosshell::desktop_native_input {

bool Channel::retained_at_exit_ = false;
Channel::Channel(flutter::BinaryMessenger* messenger, HWND view) : view_(view) {
  channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      messenger, "guosshell/desktop_native_input_v1",
      &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler([this](const auto& call, auto result) {
    HandleMethodCall(call, std::move(result));
  });
}

Channel::~Channel() {
  channel_->SetMethodCallHandler(nullptr);
  if (host_) {
    host_->DetachChannel();
    if (!host_->Closed()) retained_at_exit_ = true;
    host_.reset();
  }
  for (auto& call : pending_) call.result->Error("HOST_CLOSED", "原生输入宿主已关闭");
}
bool Channel::BeforeTranslate(const MSG& message) {
  if (!host_) return false;
  try { return host_->BeforeTranslate(message); }
  catch (...) { host_->DetachChannel(); return true; }
}
void Channel::InvalidateGeometry() { if (host_) host_->InvalidateAll(); }
void Channel::Deactivate() { if (host_) host_->Deactivate(); }
bool Channel::PrepareClose() { return !host_ || host_->Close(); }

void Channel::PollPending() {
  if (polling_ || !host_) return;
  polling_ = true;
  try {
    for (auto it = pending_.begin(); it != pending_.end();) {
      const auto now = GetTickCount64();
      if (now < it->next) { ++it; continue; }
      Map response;
      if (now >= it->deadline) {
        if (const auto* token = String(it->arguments, "paneToken")) host_->FailPane(*token, "RPC_BUSY_TIMEOUT");
        response = State(it->method == "claimAction" ? "rejected" : "failed");
      } else {
        response = host_->Call(it->method, it->arguments);
        const auto* state = String(response, "state");
        if (state && *state == "pending") { it->next = now + 16; ++it; continue; }
      }
      auto result = std::move(it->result);
      pending_bytes_ -= it->bytes;
      it = pending_.erase(it);
      result->Success(Value(std::move(response)));
    }
  } catch (...) {
    host_->DetachChannel();
    auto abandoned = std::move(pending_); pending_bytes_ = 0;
    for (auto& call : abandoned) call.result->Error("NATIVE_HOST_FAILED", "原请求无法完成，保留未确认责任");
  }
  polling_ = false;
}

void Channel::HandleMethodCall(
    const flutter::MethodCall<flutter::EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
  if (call.method_name() != "attachRuntime") {
    const auto& method = call.method_name();
    if (method != "attachPane" && method != "requestFocus" && method != "retirePane" && method != "presentedFrame" &&
        method != "invalidateGeometry" && method != "textEndRequired" && method != "textLayout" &&
        method != "claimAction" && method != "completeAction") { result->NotImplemented(); return; }
    const auto* args = call.arguments() ? std::get_if<Map>(call.arguments()) : nullptr;
    if (!args || !Version(*args)) { result->Error("BAD_ARGUMENTS", "原生窗格调用需要版本化参数"); return; }
    if (!host_) { result->Error("RUNTIME_NOT_ATTACHED", "正常输入宿主尚未关联"); return; }
    try {
      const bool retryable = method == "claimAction" || method == "completeAction" || method == "textEndRequired";
      Map saved;
      size_t bytes{};
      if (retryable) {
        // 只冻结协议字段，不把客户端额外对象复制到有界等待队列。
        for (const char* name : {"version", "paneToken", "effect", "claimToken", "kind", "text", "contextId", "transactionId", "endFence"}) {
          if (const auto* field = Find(*args, name)) {
            if (const auto* text = std::get_if<std::string>(field)) {
              if (text->size() > (std::string(name) == "text" ? 1024u * 1024u : 128u)) {
                result->Error("BAD_ARGUMENTS", "请求字段超限"); return;
              }
              bytes += text->size();
            } else if (std::string(name) != "version") { result->Error("BAD_ARGUMENTS", "请求身份字段必须为字符串"); return; }
            saved[Value(name)] = *field;
          }
        }
        if (pending_.size() >= 64 || pending_bytes_ + bytes > 16u * 1024u * 1024u) {
          if (const auto* token = String(*args, "paneToken")) host_->FailPane(*token, "RPC_CAPACITY");
          result->Success(Value(State(method == "claimAction" ? "rejected" : "failed"))); return;
        }
      }
      auto response = host_->Call(method, *args);
      const auto* state = String(response, "state");
      if (retryable && state && *state == "pending") {
        pending_.push_back({method, std::move(saved), nullptr, GetTickCount64() + 4000, GetTickCount64() + 16, bytes});
        pending_.back().result = std::move(result);
        pending_bytes_ += bytes;
      } else result->Success(Value(std::move(response)));
    }
    catch (...) {
      host_->DetachChannel();
      result->Error("NATIVE_HOST_FAILED", "正常输入宿主无法完成原请求");
    }
    return;
  }
  if (call.arguments() && !call.arguments()->IsNull()) {
    result->Error("BAD_ARGUMENTS", "attachRuntime 不接受路径或客户端身份参数");
    return;
  }
  const auto status = runtime_.Attach();
  if (status != AttachStatus::kAttached) {
    if (host_) host_->Close();
    result->Error(AttachErrorCode(status), "正常 hub 模块关联不可用");
    return;
  }
  using flutter::EncodableMap;
  using flutter::EncodableValue;
  if (!host_ && IsWindow(view_)) {
    host_ = std::make_unique<Host>(view_, *runtime_.api(), [this](const char* method, Map arguments) {
      channel_->InvokeMethod(method, std::make_unique<Value>(std::move(arguments)));
    });
    host_->SetPollCallback([this] { PollPending(); });
  }
  // 模块关联不是活会话、真实输入来源或生产 guard 的证明。
  result->Success(EncodableValue(EncodableMap{
      {EncodableValue("version"), EncodableValue(int32_t{1})},
      {EncodableValue("moduleAttached"), EncodableValue(true)},
      {EncodableValue("normalAbiVersion"), EncodableValue(int32_t{GUOSSH_NORMAL_ABI_VERSION})},
      {EncodableValue("normalExportCount"), EncodableValue(int32_t{19})},
      {EncodableValue("sameModule"), EncodableValue(true)},
      {EncodableValue("modulePinned"), EncodableValue(true)},
      {EncodableValue("inputSourceAvailable"), EncodableValue(false)},
      {EncodableValue("productionEnabled"), EncodableValue(false)},
  }));
}

}  // namespace guosshell::desktop_native_input
