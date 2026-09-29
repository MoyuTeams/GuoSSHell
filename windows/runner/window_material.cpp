#include "window_material.h"

#include <dwmapi.h>
#include <flutter/standard_method_codec.h>

namespace {

constexpr char kChannelName[] = "guosshell/window_material";
constexpr char kSetMaterial[] = "setMaterial";

// SetWindowCompositionAttribute 未在 SDK 头文件中声明，结构按 user32 的约定定义。
enum AccentState : DWORD {
  kAccentEnableGradient = 1,
  kAccentEnableAcrylicBlurBehind = 4,
};

struct AccentPolicy {
  DWORD accent_state;
  DWORD accent_flags;
  DWORD gradient_color;  // ABGR
  DWORD animation_id;
};

constexpr DWORD kWcaAccentPolicy = 19;

struct WindowCompositionAttribData {
  DWORD attribute;
  PVOID data;
  SIZE_T size;
};

using SetWindowCompositionAttributeFn =
    BOOL(WINAPI*)(HWND, WindowCompositionAttribData*);
using RtlGetVersionFn = LONG(WINAPI*)(PRTL_OSVERSIONINFOW);

// 早期 Windows 11 版本使用的未公开 Mica 开关。
constexpr DWORD kDwmwaMicaEffect = 1029;
constexpr DWORD kDwmwaSystemBackdropType = 38;
constexpr int kDwmsbtNone = 1;

DWORD WindowsBuildNumber() {
  // GetVersionEx 受兼容清单影响，直接读取系统真实版本。
  const HMODULE ntdll = ::GetModuleHandleW(L"ntdll.dll");
  if (!ntdll) return 0;
  const auto rtl_get_version = reinterpret_cast<RtlGetVersionFn>(
      ::GetProcAddress(ntdll, "RtlGetVersion"));
  if (!rtl_get_version) return 0;
  RTL_OSVERSIONINFOW info = {};
  info.dwOSVersionInfoSize = sizeof(info);
  return rtl_get_version(&info) == 0 ? info.dwBuildNumber : 0;
}

SetWindowCompositionAttributeFn LoadSetWindowCompositionAttribute() {
  const HMODULE user32 = ::GetModuleHandleW(L"user32.dll");
  if (!user32) return nullptr;
  return reinterpret_cast<SetWindowCompositionAttributeFn>(
      ::GetProcAddress(user32, "SetWindowCompositionAttribute"));
}

}  // namespace

WindowMaterial::WindowMaterial(flutter::BinaryMessenger* messenger, HWND window)
    : window_(window) {
  channel_ = std::make_unique<flutter::MethodChannel<flutter::EncodableValue>>(
      messenger, kChannelName, &flutter::StandardMethodCodec::GetInstance());
  channel_->SetMethodCallHandler([this](const auto& call, auto result) {
    HandleMethodCall(call, std::move(result));
  });
}

WindowMaterial::~WindowMaterial() {
  channel_->SetMethodCallHandler(nullptr);
}

void WindowMaterial::HandleMethodCall(
    const flutter::MethodCall<flutter::EncodableValue>& call,
    std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result) {
  if (call.method_name() != kSetMaterial) {
    result->NotImplemented();
    return;
  }
  const auto* arguments = std::get_if<flutter::EncodableMap>(call.arguments());
  if (!arguments) {
    result->Error("BAD_ARGUMENTS", "窗口材质参数缺失");
    return;
  }
  const auto acrylic = arguments->find(flutter::EncodableValue("acrylic"));
  const auto color = arguments->find(flutter::EncodableValue("color"));
  if (acrylic == arguments->end() || color == arguments->end() ||
      !std::holds_alternative<bool>(acrylic->second) ||
      !(std::holds_alternative<int32_t>(color->second) ||
        std::holds_alternative<int64_t>(color->second))) {
    result->Error("BAD_ARGUMENTS", "窗口材质参数无效");
    return;
  }
  const auto argb = static_cast<uint32_t>(color->second.LongValue());
  if (!Apply(std::get<bool>(acrylic->second), argb)) {
    result->Error("COMPOSITION_FAILED", "Windows 未能应用窗口材质");
    return;
  }
  result->Success();
}

bool WindowMaterial::Apply(bool acrylic, uint32_t argb) {
  static const auto set_window_composition_attribute =
      LoadSetWindowCompositionAttribute();
  if (!set_window_composition_attribute) return false;

  // Windows 11 的系统背景材质色调固定、不使用 alpha；先关闭它，
  // 避免与下面的 Accent 材质叠加成不透明。
  const DWORD build = WindowsBuildNumber();
  if (build >= 22523) {
    const int backdrop = kDwmsbtNone;
    ::DwmSetWindowAttribute(window_, kDwmwaSystemBackdropType, &backdrop,
                            sizeof(backdrop));
  } else if (build >= 22000) {
    const BOOL mica = FALSE;
    ::DwmSetWindowAttribute(window_, kDwmwaMicaEffect, &mica, sizeof(mica));
  }
  const MARGINS margins = {0, 0, 1, 0};
  ::DwmExtendFrameIntoClientArea(window_, &margins);

  const DWORD abgr = (argb & 0xFF000000) | ((argb & 0x000000FF) << 16) |
                     (argb & 0x0000FF00) | ((argb & 0x00FF0000) >> 16);
  AccentPolicy accent = {
      acrylic ? kAccentEnableAcrylicBlurBehind : kAccentEnableGradient, 2,
      abgr, 0};
  WindowCompositionAttribData data = {kWcaAccentPolicy, &accent,
                                      sizeof(accent)};
  return set_window_composition_attribute(window_, &data) != FALSE;
}
