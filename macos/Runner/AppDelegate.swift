import Cocoa
import FlutterMacOS

@main
class AppDelegate: FlutterAppDelegate {
  var closeChannel: FlutterMethodChannel?
  private var terminationPending = false

  override func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
    if terminationPending { return .terminateLater }
    guard let channel = closeChannel else { return .terminateCancel }
    terminationPending = true
    channel.invokeMethod("confirmClose", arguments: nil) { [weak self] result in
      self?.terminationPending = false
      // 只有工作区确认且每个会话已关闭才允许退出；桥接失败保持窗口。
      sender.reply(toApplicationShouldTerminate: (result as? Bool) == true)
    }
    return .terminateLater
  }

  override func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    return true
  }

  override func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
    return true
  }
}
