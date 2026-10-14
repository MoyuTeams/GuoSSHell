#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>
#include <algorithm>

#include "flutter_window.h"
#include "utils.h"

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  if (FAILED(::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED))) return EXIT_FAILURE;
  struct Apartment {
    ~Apartment() {
      if (guosshell::desktop_native_input::Channel::RetainedAtExit()) ::ExitProcess(EXIT_FAILURE);
      ::CoUninitialize();
    }
  } apartment;

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  const bool sdk_fixture_enabled =
      std::find(command_line_arguments.begin(), command_line_arguments.end(),
                "--guosshell-native-input-spike-sdk") != command_line_arguments.end();
  const bool source_fixture_enabled = sdk_fixture_enabled &&
      std::find(command_line_arguments.begin(), command_line_arguments.end(),
                "--guosshell-native-input-spike-source") != command_line_arguments.end();
  const bool source_exit_test_enabled = source_fixture_enabled &&
      std::find(command_line_arguments.begin(), command_line_arguments.end(),
                "--guosshell-native-input-spike-source-exit-test") != command_line_arguments.end();
  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  FlutterWindow window(project, sdk_fixture_enabled, source_fixture_enabled,
                       source_exit_test_enabled);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1280, 720);
  if (!window.Create(L"GuoSSHell", origin, size)) {
    return EXIT_FAILURE;
  }
  window.SetQuitOnClose(true);

  ::MSG msg;
  BOOL next_message = 0;
  while ((next_message = ::GetMessage(&msg, nullptr, 0, 0)) > 0) {
    if (FlutterWindow::RouteNormalInput(msg)) continue;
    const auto route = FlutterWindow::RouteOptionalFixture(msg);
    if (route == guosshell::native_input_spike::LoopRoute::Handled) continue;
    if (route == guosshell::native_input_spike::LoopRoute::ExistingRoute) ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  if (!FlutterWindow::FinishNormalInput()) {
    // 未静默的 callback/worker/root 仍保有模块和 COM；不以进程退出伪造 released。
    ::ExitProcess(EXIT_FAILURE);
  }

  const HRESULT finished = FlutterWindow::FinishOptionalFixture();
  if (source_fixture_enabled && finished != S_OK) {
    // 未确认的 Root／HWND／COM 保留至进程退出；不卸载 COM、不触发栈析构。
    ::ExitProcess(EXIT_FAILURE);
  }
  return next_message < 0 ? EXIT_FAILURE : EXIT_SUCCESS;
}
