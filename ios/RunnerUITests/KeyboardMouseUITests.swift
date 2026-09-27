// M6 D 类：iPad 上系统合成的硬件键盘与指针事件（XCUITest 走 UIKit → Flutter 引擎的真实路径），
// 远端 m6-keyecho 记下收到的每一段字节，经验收服务器的 /m6/input 核对
// （docs/acceptance-m6-2026-09-26.md §3 D）。XCUITest 在 iPad 模拟器上合成不出来的部分不在这里，
// 由集成测试从 Flutter 的事件注入（m6_protocol_test.dart）：回车、退格、Esc、Home / End、翻页、
// 向前删除到不了 App，F1–F12 到达时错一位（F2 成了 F1），悬停不产生任何指针事件。
//
// 前提：./scripts/sshd-test.sh up。运行：./scripts/m6.sh ui <设备>
import XCTest

/// Xcode 将 TEST_RUNNER_M6_* 环境变量传给测试进程；设备地址不写进代码。
private enum M6Configuration {
    static let environment = ProcessInfo.processInfo.environment
    static let host = environment["M6_HOST"] ?? "127.0.0.1"
    static let port = environment["M6_PORT"] ?? "2223"
    static var server: URL {
        var url = URLComponents()
        url.scheme = "http"
        let apiHost = environment["M6_LLM_HOST"] ?? host
        url.host = apiHost.contains(":") && !apiHost.hasPrefix("[") ? "[\(apiHost)]" : apiHost
        url.port = Int(environment["M6_LLM_PORT"] ?? "2224")
        return url.url!
    }

    static func launchEnvironment(command: String) -> [String: String] {
        ["GUOSH_HOST": host, "GUOSH_PORT": port, "GUOSH_USER": "probe",
         "GUOSH_PASS": "probe", "GUOSH_CMD": command]
    }
}

/// 只确认验收 App 和测试进程访问局域网所需的系统提示。
private func allowLocalNetworkIfAsked() {
    let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
    let alert = springboard.alerts.firstMatch
    guard alert.exists else { return }
    let networkText = alert.staticTexts.matching(NSPredicate(
        format: "label CONTAINS %@ OR label CONTAINS[c] %@", "本地网络", "local network"
    )).firstMatch
    let testAppText = alert.staticTexts.matching(NSPredicate(
        format: "label CONTAINS[c] %@ OR label CONTAINS[c] %@", "GuoSSHell", "RunnerUITests"
    )).firstMatch
    guard networkText.exists && testAppText.exists else { return }
    for label in ["允许", "Allow"] where alert.buttons[label].exists {
        alert.buttons[label].tap()
        return
    }
}

final class KeyboardMouseUITests: XCTestCase {
    private let server = M6Configuration.server
    private var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = true
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .pad, "键盘与鼠标用例只在 iPad 上跑")
    }

    override func tearDown() {
        app?.terminate()
        super.tearDown()
    }

    // MARK: - D1–D7 硬件键盘

    func testHardwareKeyboard() throws {
        try launch(modes: "")
        try ensureAsciiInput()

        // D1 可打印字符（含 Shift 大小写）与空格。
        try expectBytes("aZ09-=[];',./ ".utf8.map { $0 }) { typeKeys("aZ09-=[];',./ ") }
        try expectBytes([0x41]) { app.typeKey("a", modifierFlags: .shift) }

        // D2 Tab、Shift+Tab。
        try expectBytes([0x09]) { app.typeKey(.tab, modifierFlags: []) }
        try expectBytes(esc("[Z")) { app.typeKey(.tab, modifierFlags: .shift) }

        // D3 方向键（远端没开应用光标键：CSI 形式）。
        let arrows: [(XCUIKeyboardKey, String)] = [(.upArrow, "[A"), (.downArrow, "[B"), (.rightArrow, "[C"), (.leftArrow, "[D")]
        for (key, sequence) in arrows {
            try expectBytes(esc(sequence), "\(key.rawValue)") { app.typeKey(key, modifierFlags: []) }
        }

        // D4 Ctrl 组合：C0 控制字符（Ctrl+[ 即 Esc，妙控键盘没有实体 Esc 键时靠它）。
        let control: [(String, UInt8)] = [("a", 0x01), ("c", 0x03), ("d", 0x04), ("z", 0x1a), ("m", 0x0d), ("[", 0x1b), ("\\", 0x1c)]
        for (key, byte) in control {
            try expectBytes([byte], "ctrl+\(key)") { app.typeKey(key, modifierFlags: .control) }
        }

        // D5 Option：ESC 前缀；Option+方向键带修饰参数。
        try expectBytes(esc("b"), "option+b") { app.typeKey("b", modifierFlags: .option) }
        try expectBytes(esc("."), "option+.") { app.typeKey(".", modifierFlags: .option) }
        try expectBytes(esc("[1;3D"), "option+left") { app.typeKey(.leftArrow, modifierFlags: .option) }

        // D6 ⌘ 组合由 App 处理，不发到远端。
        try expectBytes([], "cmd+a") { app.typeKey("a", modifierFlags: .command) }
        try expectBytes([], "cmd+c") { app.typeKey("c", modifierFlags: .command) }

        // D7 连续输入 50 个键再以 Ctrl+M（CR）结尾：直接处理的键不越过前面的文本，不丢不重。
        let burst = "the quick brown fox jumps over the lazy dog 012345"
        try expectBytes(Array(burst.utf8) + [0x0d], "burst") {
            typeKeys(burst)
            app.typeKey("m", modifierFlags: .control)
        }
    }

    // MARK: - D8–D12 鼠标 / 触控板

    /// 真机生命周期：切到主屏幕后返回，原 SSH 会话仍能接收控制字符。
    func testSessionSurvivesBackground() throws {
        try launch(modes: "")
        XCUIDevice.shared.press(.home)
        let backgrounded = app.wait(for: .runningBackground, timeout: 10)
            || app.state == .runningBackgroundSuspended
        XCTAssertTrue(backgrounded, "App 应进入后台")
        Thread.sleep(forTimeInterval: 5)
        app.activate()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 10), "App 应回到前台")
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2)).tap()
        try expectBytes([0x0d], "后台恢复") { app.typeKey("m", modifierFlags: .control) }
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "后台恢复"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testPointerClicksDragsAndWheel() throws {
        try launch(modes: "mouse drag")
        let center = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2))
        let lower = app.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.28))

        // D8 单击、右键、双击：SGR 按下 / 松开（按钮 0 左、2 右）。
        var events = try mouseEvents { center.click() }
        XCTAssertEqual(events.map(\.kind), ["0M", "0m"], "单击：\(events)")
        events = try mouseEvents { center.rightClick() }
        XCTAssertEqual(events.map(\.kind), ["2M", "2m"], "右键：\(events)")
        events = try mouseEvents { center.doubleClick() }
        XCTAssertEqual(events.map(\.kind), ["0M", "0m", "0M", "0m"], "双击：\(events)")

        // D9 按住拖动：按下、若干拖动（按钮 0 + 32）、松开，列 / 行跟着移动。
        events = try mouseEvents { center.click(forDuration: 0.3, thenDragTo: lower) }
        XCTAssertEqual(events.first?.kind, "0M", "拖动的开始：\(events)")
        XCTAssertEqual(events.last?.kind, "0m", "拖动的结束：\(events)")
        XCTAssertTrue(events.contains { $0.kind == "32M" }, "拖动过程中要有移动上报：\(events)")
        if let first = events.first, let last = events.last {
            XCTAssertTrue(last.col > first.col && last.row > first.row, "拖到右下：\(first) → \(last)")
        }

        // D11 滚轮 / 触控板：全屏程序开了鼠标上报时是滚轮事件（64 上、65 下）。
        events = try mouseEvents { center.scroll(byDeltaX: 0, deltaY: -120) }
        XCTAssertFalse(events.isEmpty, "滚动要上报")
        XCTAssertTrue(events.allSatisfy { $0.kind == "64M" || $0.kind == "65M" }, "滚轮：\(events)")

        // D12 修饰键：Option（8）、Control（16）；Shift+点击走本地选区、不上报。
        events = try mouseEvents { XCUIElement.perform(withKeyModifiers: .option) { center.click() } }
        XCTAssertEqual(events.first?.kind, "8M", "Option+点击：\(events)")
        events = try mouseEvents { XCUIElement.perform(withKeyModifiers: .control) { center.click() } }
        XCTAssertTrue(events.first.map { $0.kind == "16M" || $0.kind == "18M" } ?? false, "Control+点击：\(events)")
        events = try mouseEvents { XCUIElement.perform(withKeyModifiers: .shift) { center.click() } }
        XCTAssertTrue(events.isEmpty, "Shift+点击是本地选区，不发到远端：\(events)")
    }

    /// 编辑器可在真实会话中打开和保存，当前排布保持不变。
    func testKeyBarEditor() throws {
        try launch(modes: "mouse drag")
        let edit = app.buttons["编辑功能按钮"]
        XCTAssertTrue(edit.waitForExistence(timeout: 10))
        edit.tap()
        XCTAssertTrue(app.staticTexts["添加按钮"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["恢复默认"].exists)
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.name = "功能按钮编辑器"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        app.buttons["保存"].tap()
        XCTAssertTrue(edit.waitForExistence(timeout: 10))
    }

    /// 修饰键回归独立于 XCTest 的滚轮合成限制。
    func testModifierCombinations() throws {
        try launch(modes: "mouse drag")
        try ensureAsciiInput()
        try expectBytes([0x1b], "Ctrl+[") { app.typeKey("[", modifierFlags: .control) }
        try expectBytes([0x1b, 0x78], "Option+x") { app.typeKey("x", modifierFlags: .option) }
        try expectBytes([0x1b, 0x5b, 0x5a], "Shift+Tab") { app.typeKey(XCUIKeyboardKey.tab.rawValue, modifierFlags: .shift) }
        let center = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2))
        var events = try mouseEvents { XCUIElement.perform(withKeyModifiers: .option) { center.click() } }
        XCTAssertEqual(events.first?.kind, "8M", "Option+点击")
        events = try mouseEvents { XCUIElement.perform(withKeyModifiers: .control) { center.click() } }
        XCTAssertTrue(events.first.map { $0.kind == "16M" || $0.kind == "18M" } ?? false, "Control+点击")
        events = try mouseEvents { XCUIElement.perform(withKeyModifiers: .shift) { center.click() } }
        XCTAssertTrue(events.isEmpty, "Shift+点击留在本地")
        try expectBytes([], "⌘+缩放不发送远端") { app.typeKey("=", modifierFlags: .command) }
        try expectBytes([], "⌘0重置不发送远端") { app.typeKey("0", modifierFlags: .command) }
        try expectBytes([0x61], "松开修饰键后普通文本") { app.typeKey("a", modifierFlags: []) }
    }

    // MARK: - 辅助

    /// 启动 App，自动连上验收服务器并以 exec 模式运行 m6-keyecho；点一下终端拿到焦点，
    /// 再用探测键等回显程序就绪。
    private func launch(modes: String) throws {
        app = XCUIApplication()
        app.launchM6(command: "m6-keyecho \(modes) --seconds 900")
        guard app.waitConnected() else {
            XCTFail("App 没连上验收服务器")
            throw XCTSkip("未连接")
        }
        // XCUITest 每个动作前等 App 空闲，最多 60 秒；给就绪留出几轮的余量。
        let deadline = Date().addingTimeInterval(180)
        repeat {
            app.acceptHostKeyIfAsked()
            try resetLog()
            app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2)).tap()
            app.typeKey("~", modifierFlags: [])
            Thread.sleep(forTimeInterval: 1.0)
            if try readLog().contains(0x7e) {
                try resetLog()
                return
            }
        } while Date() < deadline
        XCTFail("m6-keyecho 没有就绪")
        throw XCTSkip("回显程序未就绪")
    }

    /// 字节断言需要英文输入；用回显核对，并通过系统切换键切换输入法，不修改键盘配置。
    private func ensureAsciiInput() throws {
        for _ in 0..<4 {
            try resetLog()
            app.typeKey("a", modifierFlags: [])
            Thread.sleep(forTimeInterval: 0.4)
            if try readLog() == [0x61] {
                try resetLog()
                return
            }
            app.typeKey(" ", modifierFlags: .control)
            Thread.sleep(forTimeInterval: 0.5)
        }
        throw XCTSkip("系统输入法没有进入英文，原始按键字节断言需要英文输入法")
    }

    /// 逐键敲出 [text]（大写字母带 Shift）。终端的文本输入不在无障碍树里，XCUITest 的 typeText
    /// 找不到键盘焦点，只能逐键合成硬件按键。
    private func typeKeys(_ text: String) {
        for character in text {
            if character.isUppercase {
                app.typeKey(character.lowercased(), modifierFlags: .shift)
            } else {
                app.typeKey(String(character), modifierFlags: [])
            }
        }
    }

    /// 做 [action]，核对远端收到的字节正好是 [expected]。
    private func expectBytes(_ expected: [UInt8], _ label: String = "", file: StaticString = #filePath, line: UInt = #line,
                             _ action: () -> Void) throws {
        try resetLog()
        action()
        let received = try settleLog()
        XCTAssertEqual(hex(received), hex(expected), "\(label) 收到 \(hex(received))", file: file, line: line)
    }

    /// 做 [action]，解出远端收到的 SGR 鼠标事件。
    private func mouseEvents(_ action: () -> Void) throws -> [MouseEvent] {
        try resetLog()
        action()
        let text = String(decoding: try settleLog(), as: UTF8.self)
        let pattern = try NSRegularExpression(pattern: "\u{1b}\\[<(\\d+);(\\d+);(\\d+)([Mm])")
        return pattern.matches(in: text, range: NSRange(text.startIndex..., in: text)).map { match in
            func group(_ index: Int) -> String { String(text[Range(match.range(at: index), in: text)!]) }
            return MouseEvent(kind: group(1) + group(4), col: Int(group(2)) ?? 0, row: Int(group(3)) ?? 0)
        }
    }

    /// 等记录静下来（最后一次写入后 0.6 秒没有新字节）。
    private func settleLog() throws -> [UInt8] {
        var last = try readLog()
        var quiet = 0
        for _ in 0..<40 where quiet < 3 {
            Thread.sleep(forTimeInterval: 0.2)
            let now = try readLog()
            quiet = now == last ? quiet + 1 : 0
            last = now
        }
        return last
    }

    private func resetLog() throws {
        var request = URLRequest(url: server.appendingPathComponent("m6/input/reset"))
        request.httpMethod = "POST"
        _ = try fetch(request)
    }

    /// 回显程序记下的字节（每行一条 JSON：{"t": 毫秒, "hex": "..."}），按顺序拼起来。
    private func readLog() throws -> [UInt8] {
        let data = try fetch(URLRequest(url: server.appendingPathComponent("m6/input")))
        var bytes: [UInt8] = []
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
            guard let json = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any],
                  let hexText = json["hex"] as? String else { continue }
            var index = hexText.startIndex
            while index < hexText.endIndex {
                let next = hexText.index(index, offsetBy: 2)
                bytes.append(UInt8(hexText[index..<next], radix: 16) ?? 0)
                index = next
            }
        }
        return bytes
    }

    private func fetch(_ request: URLRequest) throws -> Data {
        let done = DispatchSemaphore(value: 0)
        var result: Result<Data, Error> = .failure(URLError(.timedOut))
        URLSession.shared.dataTask(with: request) { data, _, error in
            result = error.map { .failure($0) } ?? .success(data ?? Data())
            done.signal()
        }.resume()
        let deadline = Date().addingTimeInterval(10)
        while done.wait(timeout: .now() + 0.2) == .timedOut && Date() < deadline {
            allowLocalNetworkIfAsked()
        }
        return try result.get()
    }

    private func esc(_ sequence: String) -> [UInt8] { [0x1b] + Array(sequence.utf8) }

    private func hex(_ bytes: [UInt8]) -> String { bytes.map { String(format: "%02x", $0) }.joined(separator: " ") }
}

private struct MouseEvent: CustomStringConvertible {
    /// 按钮码加结尾（M 按下 / 移动，m 松开），如 "0M"、"32M"、"0m"。
    let kind: String
    let col: Int
    let row: Int

    var description: String { "\(kind)@\(col),\(row)" }
}

/// M6 B3 / C：真实旋转（系统转动设备）下的全屏 TUI。画面内容由截图复核（附在测试结果里），
/// 断言只管 App 活着、没有弹出异常界面。iPhone 与 iPad 都跑。
final class RotationUITests: XCTestCase {
    func testFullScreenTUIAcrossRotations() throws {
        let app = XCUIApplication()
        XCUIDevice.shared.orientation = .portrait
        app.launchM6(command: "m6-tui btop")
        XCTAssertTrue(app.waitConnected(), "App 没连上验收服务器")
        Thread.sleep(forTimeInterval: 6)
        attach("竖屏")
        for orientation in [UIDeviceOrientation.landscapeLeft, .portrait, .landscapeRight, .portrait] {
            XCUIDevice.shared.orientation = orientation
            Thread.sleep(forTimeInterval: 4)
            attach(orientation.isLandscape ? "横屏" : "竖屏")
            XCTAssertEqual(app.state, .runningForeground)
        }
        app.terminate()
    }

    private func attach(_ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}

extension XCUIApplication {
    /// 真机的 profile 构建走实际快速连接表单；模拟器保留 debug 自动连接。
    func launchM6(command: String) {
        launchEnvironment = M6Configuration.launchEnvironment(command: command)
        launch()
        guard M6Configuration.environment["M6_USE_FORM"] == "1" else { return }
        let quick = buttons["快速连接"]
        XCTAssertTrue(quick.waitForExistence(timeout: 20), "应显示连接列表")
        quick.tap()
        func fill(_ label: String, _ value: String) {
            let predicate = NSPredicate(format: "label BEGINSWITH %@", label)
            // Flutter 密码框聚焦时的无障碍类型会变化，按标签查找以保持定位稳定。
            let field = descendants(matching: .any).matching(predicate).firstMatch
            XCTAssertTrue(field.waitForExistence(timeout: 10), "应显示表单项：\(label)")
            // 妙控键盘下，数字输入会弹出悬浮数字键盘；先关闭它，再切换到下一项。
            let dismiss = otherElements["PopoverDismissRegion"]
            if dismiss.exists { dismiss.tap() }
            // Flutter 文本框有时被 AX 标为不可点击；用它的实际边界发送系统点按。
            let previous = field.value as? String ?? ""
            field.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
            typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: previous.count) + value)
        }
        fill("主机", M6Configuration.host)
        fill("端口", M6Configuration.port)
        fill("用户名", "probe")
        fill("密码", "probe")
        fill("命令", command)
        buttons["连接"].tap()
    }

    /// 首次连接或服务器换了主机密钥时 App 请用户确认——验收服务器是自己起的，照单信任。
    /// 点掉了对话框返回 true。
    @discardableResult
    func acceptHostKeyIfAsked() -> Bool {
        let trust = buttons["信任并连接"]
        if trust.exists {
            trust.tap()
            return true
        }
        let replace = buttons["替换旧密钥并连接"]
        if replace.exists {
            labeled("label CONTAINS %@", "核对新指纹").tap()
            replace.tap()
            return true
        }
        return false
    }

    /// 等 App 连上验收服务器：连续 3 秒终端标签在、没有「正在连接」、没有确认对话框（刚启动时
    /// 标签先于「正在连接」出现，只看一眼会误判）。
    func waitConnected(timeout: TimeInterval = 60) -> Bool {
        let connecting = labeled("label BEGINSWITH %@", "正在连接")
        let tab = labeled("label CONTAINS %@", "probe@")
        let deadline = Date().addingTimeInterval(timeout)
        var settled = 0
        var retries = 0
        repeat {
            allowLocalNetworkIfAsked()
            if acceptHostKeyIfAsked() {
                settled = 0
            } else if buttons["重试"].exists {
                settled = 0
                if retries < 3 {
                    retries += 1
                    buttons["重试"].tap()
                }
            } else if tab.exists && !connecting.exists {
                settled += 1
                if settled >= 6 { return true }
            } else {
                settled = 0
            }
            Thread.sleep(forTimeInterval: 0.5)
        } while Date() < deadline
        return false
    }

    private func labeled(_ format: String, _ text: String) -> XCUIElement {
        descendants(matching: .any).matching(NSPredicate(format: format, text)).firstMatch
    }
}
