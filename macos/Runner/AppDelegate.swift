import Cocoa
import FlutterMacOS

@main
class AppDelegate: FlutterAppDelegate {
  var closeChannel: FlutterMethodChannel? {
    didSet {
      closeChannel?.setMethodCallHandler { [weak self] call, result in
        guard call.method == "confirmClosePending" else {
          result(FlutterMethodNotImplemented)
          return
        }
        // Dart 已收到退出请求并在询问用户：取消无响应兜底，等待最终答复不设时限。
        self?.fallback?.cancel()
        self?.fallback = nil
        result(nil)
      }
    }
  }
  private var terminationPending = false
  private var fallback: DispatchWorkItem?

  override func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    // 首个退出确认仍在进行，由它给出最终答复。
    if terminationPending { return .terminateCancel }
    // 桥接未建立时不能把用户困在应用里。
    guard let channel = closeChannel else { return .terminateNow }
    terminationPending = true
    var replied = false
    let finish: (Bool) -> Void = { [weak self] allow in
      if replied { return }
      replied = true
      self?.fallback?.cancel()
      self?.fallback = nil
      self?.terminationPending = false
      sender.reply(toApplicationShouldTerminate: allow)
    }
    // Dart 未在 15 秒内确认收到时视为无响应，放行退出。
    let timeout = DispatchWorkItem { finish(true) }
    fallback = timeout
    // 确认框不能藏在最小化或后台的窗口后面。
    NSApp.activate(ignoringOtherApps: true)
    channel.invokeMethod("confirmClose", arguments: nil) { result in
      // 只有工作区明确答复 false 才取消；未实现、异常或空结果都放行退出。
      if let ok = result as? Bool, !ok {
        finish(false)
      } else {
        finish(true)
      }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 15, execute: timeout)
    return .terminateLater
  }

  override func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    return true
  }

  override func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
    return true
  }
}
