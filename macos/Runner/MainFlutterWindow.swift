import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)

    (NSApplication.shared.delegate as? AppDelegate)?.closeChannel = FlutterMethodChannel(
      name: "guosshell/window_close",
      binaryMessenger: flutterViewController.engine.binaryMessenger
    )

    super.awakeFromNib()
  }

  override func performClose(_ sender: Any?) {
    // 最后一个窗口与 Cmd+Q 共用异步会话关闭门禁。
    NSApplication.shared.terminate(sender)
  }
}
