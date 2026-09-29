#ifndef RUNNER_WINDOW_MATERIAL_H_
#define RUNNER_WINDOW_MATERIAL_H_

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>
#include <windows.h>

#include <memory>

// 窗口材质：接受色调 alpha 的亚克力或纯色背景，由 Dart 经方法通道
// `guosshell/window_material` 切换。透明度由 Flutter 背景遮罩控制，
// 原生层只负责模糊与底色。
class WindowMaterial {
 public:
  WindowMaterial(flutter::BinaryMessenger* messenger, HWND window);
  ~WindowMaterial();

  WindowMaterial(const WindowMaterial&) = delete;
  WindowMaterial& operator=(const WindowMaterial&) = delete;

 private:
  void HandleMethodCall(
      const flutter::MethodCall<flutter::EncodableValue>& call,
      std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result);

  // 成功应用返回 true；系统缺少合成接口或调用失败返回 false。
  bool Apply(bool acrylic, uint32_t argb);

  HWND window_;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
};

#endif  // RUNNER_WINDOW_MATERIAL_H_
