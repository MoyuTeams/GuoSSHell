#include "channel.h"

#include <flutter/standard_method_codec.h>

#include <cstdio>
#include <string>
#include <vector>

namespace {
using flutter::EncodableMap;
using flutter::EncodableValue;

class Messenger : public flutter::BinaryMessenger {
 public:
  void Send(const std::string&, const uint8_t*, size_t,
            flutter::BinaryReply) const override {}
  void SetMessageHandler(const std::string& name,
                         flutter::BinaryMessageHandler handler) override {
    if (name == "guosshell/desktop_native_input_v1") handler_ = std::move(handler);
  }
  flutter::BinaryMessageHandler handler_;
};

class Reply : public flutter::MethodResult<EncodableValue> {
 public:
  void SuccessInternal(const EncodableValue* value) override {
    success = true;
    if (value) result = *value;
  }
  void ErrorInternal(const std::string& code, const std::string&,
                     const EncodableValue*) override { error = code; }
  void NotImplementedInternal() override { not_implemented = true; }
  bool success = false;
  bool not_implemented = false;
  std::string error;
  EncodableValue result;
};

bool Call(Messenger& messenger, const std::string& name,
          const EncodableValue* arguments, Reply* reply) {
  const auto& codec = flutter::StandardMethodCodec::GetInstance();
  auto call = codec.EncodeMethodCall(flutter::MethodCall<EncodableValue>(
      name, arguments ? std::make_unique<EncodableValue>(*arguments) : nullptr));
  bool replied = false;
  bool decoded = false;
  messenger.handler_(call->data(), call->size(), [&](const uint8_t* data, size_t size) {
    replied = true;
    if (size == 0) {
      reply->NotImplemented();
      decoded = true;
    } else {
      decoded = codec.DecodeAndProcessResponseEnvelope(data, size, reply);
    }
  });
  return replied && decoded;
}

bool Field(const EncodableMap& map, const char* key, const EncodableValue& value) {
  const auto found = map.find(EncodableValue(key));
  return found != map.end() && found->second == value;
}
}  // namespace

int wmain(int argc, wchar_t** argv) {
  HMODULE module = nullptr;
  if (argc == 2) {
    module = LoadLibraryExW(argv[1], nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!module) return 1;
  }
  Messenger messenger;
  {
    guosshell::desktop_native_input::Channel channel(&messenger);
    if (!messenger.handler_) return 1;
    const EncodableValue injected(EncodableMap{
        {EncodableValue("path"), EncodableValue("hub.dll")},
        {EncodableValue("window"), EncodableValue(int64_t{1})}});
    Reply rejected;
    if (!Call(messenger, "attachRuntime", &injected, &rejected) ||
        rejected.error != "BAD_ARGUMENTS" || rejected.success) return 1;
    Reply unknown;
    if (!Call(messenger, "attachPane", nullptr, &unknown) ||
        unknown.error != "BAD_ARGUMENTS") return 1;
    Reply attached;
    if (!Call(messenger, "attachRuntime", nullptr, &attached)) return 1;
    if (!module) {
      if (attached.success || attached.error != "RUNTIME_MISSING") return 1;
    } else {
      const auto* result = std::get_if<EncodableMap>(&attached.result);
      if (!attached.success || !result ||
          !Field(*result, "version", EncodableValue(int32_t{1})) ||
          !Field(*result, "moduleAttached", EncodableValue(true)) ||
          !Field(*result, "sameModule", EncodableValue(true)) ||
          !Field(*result, "modulePinned", EncodableValue(true)) ||
          !Field(*result, "inputSourceAvailable", EncodableValue(false)) ||
          !Field(*result, "productionEnabled", EncodableValue(false))) return 1;
    }
  }
  if (messenger.handler_) return 1;
  if (module) FreeLibrary(module);
  std::printf("通过：私有 MethodChannel 编解码与权限边界\n");
  return 0;
}
