#include "flutter_window.h"

#include <optional>
#include <cstdio>

#include "flutter/generated_plugin_registrant.h"

namespace {
constexpr UINT_PTR kSourceCloseTimer = 0x4e4953;
constexpr UINT_PTR kNormalCloseTimer = 0x4e494e;
}

FlutterWindow* FlutterWindow::source_fixture_window_ = nullptr;
FlutterWindow* FlutterWindow::normal_window_ = nullptr;

FlutterWindow::FlutterWindow(const flutter::DartProject& project,
                             bool sdk_fixture_enabled, bool source_fixture_enabled,
                             bool source_exit_test_enabled)
    : project_(project), sdk_fixture_enabled_(sdk_fixture_enabled),
      source_fixture_enabled_(sdk_fixture_enabled && source_fixture_enabled),
      source_exit_test_enabled_(source_fixture_enabled_ && source_exit_test_enabled) {}

FlutterWindow::~FlutterWindow() {
  if (normal_window_ == this) normal_window_ = nullptr;
  if (source_fixture_window_ == this) source_fixture_window_ = nullptr;
}
bool FlutterWindow::RouteNormalInput(const MSG& message) noexcept {
  try { return normal_window_ && normal_window_->desktop_native_input_ && normal_window_->desktop_native_input_->BeforeTranslate(message); }
  catch (...) { return true; }
}
bool FlutterWindow::FinishNormalInput() noexcept {
  if (guosshell::desktop_native_input::Channel::RetainedAtExit()) return false;
  try { return !normal_window_ || !normal_window_->desktop_native_input_ || normal_window_->desktop_native_input_->PrepareClose(); }
  catch (...) { return false; }
}

guosshell::native_input_spike::LoopRoute FlutterWindow::RouteOptionalFixture(const MSG& message) noexcept {
  NativeInputSdkChannel::PollRetainedSource();
  auto* const window = source_fixture_window_;
  return window && window->sdk_fixture_channel_
      ? window->sdk_fixture_channel_->BeforeTranslate(message).route
      : guosshell::native_input_spike::LoopRoute::ExistingRoute;
}

HRESULT FlutterWindow::FinishOptionalFixture() noexcept {
  auto* const window = source_fixture_window_;
  HRESULT active = S_OK;
  if (window && window->sdk_fixture_channel_) {
    if (window->source_close_waiting_) KillTimer(window->GetHandle(), kSourceCloseTimer);
    window->source_close_waiting_ = false;
    active = window->sdk_fixture_channel_->PrepareForProcessExit();
  }
  const HRESULT retained = NativeInputSdkChannel::ReportRetainedAtExit();
  if (retained != S_OK) return retained;
  return window && window->sdk_fixture_channel_
      ? window->sdk_fixture_channel_->CloseForWindow() : active;
}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  window_material_ = std::make_unique<WindowMaterial>(
      flutter_controller_->engine()->messenger(), GetHandle());
  desktop_native_input_ =
      std::make_unique<guosshell::desktop_native_input::Channel>(
          flutter_controller_->engine()->messenger(), flutter_controller_->view()->GetNativeWindow());
  normal_window_ = this;
  if (sdk_fixture_enabled_) {
    sdk_fixture_channel_ = std::make_unique<NativeInputSdkChannel>(
        flutter_controller_->engine()->messenger(), GetHandle(), source_fixture_enabled_,
        source_exit_test_enabled_);
    if (source_fixture_enabled_) source_fixture_window_ = this;
  }
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  flutter_controller_->engine()->SetNextFrameCallback([&]() {
    this->Show();
  });

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  if (normal_close_waiting_) KillTimer(GetHandle(), kNormalCloseTimer);
  if (normal_window_ == this) normal_window_ = nullptr;
  if (source_close_waiting_) KillTimer(GetHandle(), kSourceCloseTimer);
  if (source_fixture_window_ == this) source_fixture_window_ = nullptr;
  sdk_fixture_channel_ = nullptr;
  desktop_native_input_ = nullptr;
  window_material_ = nullptr;
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // 在父 HWND 和 Flutter 尚存时退休 source；每次仅做非阻塞 UI poll。
  if (source_fixture_enabled_ && sdk_fixture_channel_ &&
      (message == WM_CLOSE || (message == WM_TIMER && wparam == kSourceCloseTimer && source_close_waiting_))) {
    if (source_close_failed_) return 0;
    const HRESULT close = sdk_fixture_channel_->CloseForWindow();
    if (close != S_OK) {
      if (close != E_PENDING) {
        if (source_close_waiting_) KillTimer(hwnd, kSourceCloseTimer);
        source_close_waiting_ = false;
        source_close_failed_ = true;
        std::fprintf(stderr, "[source-fixture] closeHr=%ld retained=1 parentCloseDisabled=1 processExitPin=1\n", static_cast<long>(close));
      } else if (!source_close_waiting_) {
        source_close_waiting_ = SetTimer(hwnd, kSourceCloseTimer, 16, nullptr) != 0;
        if (!source_close_waiting_) {
          source_close_failed_ = true;
          std::fprintf(stderr, "[source-fixture] closeHr=%ld timer=0 retained=1 parentCloseDisabled=1 processExitPin=1\n", static_cast<long>(close));
        }
      }
      return 0;
    }
    if (source_close_waiting_) {
      KillTimer(hwnd, kSourceCloseTimer);
      source_close_waiting_ = false;
    }
    if (message == WM_TIMER) {
      PostMessageW(hwnd, WM_CLOSE, 0, 0);
      return 0;
    }
  }
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (desktop_native_input_ && ((message == WM_ACTIVATEAPP && !wparam) || message == WM_CANCELMODE)) {
    desktop_native_input_->Deactivate();
  }
  if (desktop_native_input_ && (message == WM_MOVE || message == WM_SIZE || message == WM_DPICHANGED)) {
    desktop_native_input_->InvalidateGeometry();
  }
  if (desktop_native_input_ && message == WM_TIMER && wparam == kNormalCloseTimer && normal_close_waiting_) {
    if (desktop_native_input_->PrepareClose()) {
      KillTimer(hwnd, kNormalCloseTimer); normal_close_waiting_ = false; PostMessageW(hwnd, WM_CLOSE, 0, 0);
    } else if (GetTickCount64() >= normal_close_deadline_) {
      KillTimer(hwnd, kNormalCloseTimer); normal_close_waiting_ = false;
      std::fprintf(stderr, "[normal-input] 关闭未完成，保留原 callback/claim/root，不销毁 COM\n");
    }
    return 0;
  }
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  // window_manager 的用户确认先执行；实际销毁必须等 nativequiet 与原 claim/root 结算。
  if (message == WM_CLOSE && desktop_native_input_ && !desktop_native_input_->PrepareClose()) {
    if (!normal_close_waiting_) {
      normal_close_deadline_ = GetTickCount64() + 5000;
      normal_close_waiting_ = SetTimer(hwnd, kNormalCloseTimer, 16, nullptr) != 0;
    }
    return 0;
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
