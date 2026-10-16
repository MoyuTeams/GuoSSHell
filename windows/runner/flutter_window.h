#ifndef RUNNER_FLUTTER_WINDOW_H_
#define RUNNER_FLUTTER_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>

#include <memory>

#include "win32_window.h"
#include "window_material.h"
#include "native_input_spike_sdk_channel.h"
#include "desktop_native_input/channel.h"

// A window that does nothing but host a Flutter view.
class FlutterWindow : public Win32Window {
 public:
  // Creates a new FlutterWindow hosting a Flutter view running |project|.
  explicit FlutterWindow(const flutter::DartProject& project,
                         bool sdk_fixture_enabled = false,
                         bool source_fixture_enabled = false,
                         bool source_exit_test_enabled = false);
  virtual ~FlutterWindow();
  // 只路由全 app 窗口对象的可选夹具，必须先于 TranslateMessage／Flutter 分发。
  static guosshell::native_input_spike::LoopRoute RouteOptionalFixture(const MSG&) noexcept;
  static HRESULT FinishOptionalFixture() noexcept;
  static bool RouteNormalInput(const MSG&) noexcept;
  static bool FinishNormalInput() noexcept;

 protected:
  // Win32Window:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window, UINT const message, WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  // The project to run.
  flutter::DartProject project_;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  // 应用自有的窗口材质通道，须先于 Flutter 实例销毁。
  std::unique_ptr<WindowMaterial> window_material_;

  // 正常输入私有通道只关联 Rinf 已加载的模块，不启动另一份运行时。
  std::unique_ptr<guosshell::desktop_native_input::Channel> desktop_native_input_;
  static FlutterWindow* normal_window_;
  bool normal_close_waiting_ = false;
  ULONGLONG normal_close_deadline_ = 0;

  const bool sdk_fixture_enabled_;
  const bool source_fixture_enabled_;
  const bool source_exit_test_enabled_;
  static FlutterWindow* source_fixture_window_;
  bool source_close_waiting_ = false;
  bool source_close_failed_ = false;
  std::unique_ptr<NativeInputSdkChannel> sdk_fixture_channel_;
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
