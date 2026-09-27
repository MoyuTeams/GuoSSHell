#!/usr/bin/env bash
# M6 验收的编排（docs/acceptance-m6-2026-09-26.md）：模拟器矩阵、集成测试、报告汇总。
# 先 ./scripts/sshd-test.sh up 起验收服务器。
#
#   ./scripts/m6.sh sims                        建 / 找四台模拟器，打印 设备名 → udid
#   ./scripts/m6.sh run <套件> <设备> [flutter drive 参数…]
#                                               在一台模拟器上跑一套（protocol / tui / agents）
#   ./scripts/m6.sh matrix [套件…]              四台模拟器依次跑（默认三套全跑）
#   ./scripts/m6.sh ui <设备>                   XCUITest：iPad 上系统合成的硬件键盘与指针事件（D 类），
#                                               各设备上真实旋转下的全屏 TUI
#   ./scripts/m6.sh macos <套件> [参数…]        macOS 的 profile 构建上跑（性能门槛在这里判）
#   ./scripts/m6.sh device <套件> <udid> [参数…] 真机集成测试，默认 profile；M6_MODE=debug 可改为功能验证
#   ./scripts/m6.sh ui-device <udid> [参数…]    真机 XCUITest，额外参数传给 xcodebuild
#   ./scripts/m6.sh report                      汇总 build/m6/reports/*.json → build/m6/summary.md
#
# M6_REPORT=<名字> 改报告与日志的名字（默认 <套件>-<设备>）：只补跑一部分时不覆盖整套的结果。
# 真机必填 M6_HOST；M6_PORT / M6_LLM_HOST / M6_LLM_PORT 指定测试台，M6_DEVICE 指定报告标签。
# 本机签名覆盖放在不入库的 xcconfig，经 XCODE_XCCONFIG_FILE 指定。
#
# 设备：iphone-17-pro-max、iphone-17、ipad-pro-13、ipad-pro-11（iOS 模拟器，没有就按机型新建）。
# 模拟器跑在主机上，主机忙时帧率与计时都会失真（PLAN 陷阱 36）：构建之后、测试之前等 1 分钟
# 负载降到 M6_LOAD_MAX（默认 CPU 核数 × 1.5）以下，最多等 M6_LOAD_WAIT 秒（默认 300）。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${HERE}/.." && pwd)"
cd "${ROOT}"

for dir in "${HOME}/fvm/default/bin" "${HOME}/.cargo/bin"; do
  [ -d "${dir}" ] && PATH="${dir}:${PATH}"
done
export PATH

DEVICES=(iphone-17-pro-max iphone-17 ipad-pro-13 ipad-pro-11)
SUITES=(protocol tui agents)

# 调试服务和验收服务器走直连，避免本机 HTTP 代理拦截回环与局域网请求。
m6_no_proxy="${no_proxy:-${NO_PROXY:-}}"
m6_no_proxy="${m6_no_proxy:+${m6_no_proxy},}localhost,127.0.0.1,::1,${M6_HOST:-127.0.0.1}"
export no_proxy="${m6_no_proxy}" NO_PROXY="${m6_no_proxy}"

usage() {
  sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 2
}

device_type() {
  case "$1" in
    iphone-17-pro-max) echo "com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro-Max" ;;
    iphone-17) echo "com.apple.CoreSimulator.SimDeviceType.iPhone-17" ;;
    ipad-pro-13) echo "com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M5-12GB" ;;
    ipad-pro-11) echo "com.apple.CoreSimulator.SimDeviceType.iPad-Pro-11-inch-M5-12GB" ;;
    *) echo "未知设备：$1（可选：${DEVICES[*]}）" >&2; exit 2 ;;
  esac
}

# 最新的 iOS runtime。
runtime() {
  xcrun simctl list runtimes available -j | python3 -c '
import json, sys
runtimes = [r for r in json.load(sys.stdin)["runtimes"] if r["platform"] == "iOS" and r["isAvailable"]]
runtimes.sort(key=lambda r: [int(x) for x in r["version"].split(".")])
print(runtimes[-1]["identifier"])'
}

# 设备名 GuoSSH-M6 <设备>：已有就用，没有就按机型新建。
udid_of() {
  local name="GuoSSH-M6 $1" type
  type="$(device_type "$1")"
  local existing
  existing="$(xcrun simctl list devices available -j | python3 -c '
import json, sys
name = sys.argv[1]
for devices in json.load(sys.stdin)["devices"].values():
    for d in devices:
        if d["name"] == name:
            print(d["udid"]); raise SystemExit
' "${name}")"
  if [ -n "${existing}" ]; then
    echo "${existing}"
  else
    xcrun simctl create "${name}" "${type}" "$(runtime)"
  fi
}

# D 类核对的是原始按键字节：验收模拟器只留英文（美国）键盘。新建的模拟器跟主机的语言，中文时默认是
# 拼音，字母会被组成汉字。改设备自己的偏好文件（要在关机时改；不能用主机的 defaults，它会把
# .GlobalPreferences 当成主机的全局域）。
english_keyboard() {
  local udid="$1"
  local prefs="${HOME}/Library/Developer/CoreSimulator/Devices/${udid}/data/Library/Preferences/.GlobalPreferences.plist"
  local keyboard="en_US@sw=QWERTY;hw=Automatic"
  # 已经只有英文（系统开机后可能自己加回 emoji 键盘，不影响按键）就不动。
  if [ -f "${prefs}" ] && plutil -extract AppleKeyboards json -o - "${prefs}" 2>/dev/null |
    grep -Eq '^\["en_US@[^"]*"(,"emoji@[^"]*")?\]$'; then
    return
  fi
  xcrun simctl shutdown "${udid}" 2>/dev/null || true
  if [ ! -f "${prefs}" ]; then
    # 从没启动过：先启动一次让系统建好偏好文件。
    xcrun simctl boot "${udid}" 2>/dev/null || true
    xcrun simctl bootstatus "${udid}" -b >/dev/null
    xcrun simctl shutdown "${udid}"
  fi
  plutil -replace AppleKeyboards -json "[\"${keyboard}\"]" "${prefs}"
  plutil -replace AppleKeyboardsExpanded -integer 1 "${prefs}"
}

boot() {
  local udid="$1"
  english_keyboard "${udid}"
  xcrun simctl boot "${udid}" 2>/dev/null || true
  xcrun simctl bootstatus "${udid}" -b >/dev/null
}

wait_for_load() {
  local max="${M6_LOAD_MAX:-$(( $(sysctl -n hw.ncpu) * 3 / 2 ))}" waited=0 limit="${M6_LOAD_WAIT:-300}"
  while :; do
    local load
    load="$(sysctl -n vm.loadavg | awk '{print $2}')"
    if awk "BEGIN{exit !(${load} < ${max})}"; then
      echo "主机负载 ${load}（< ${max}）"
      return
    fi
    if [ "${waited}" -ge "${limit}" ]; then
      echo "主机负载 ${load} 仍高于 ${max}，照常开始（结果里的计时可能失真）" >&2
      return
    fi
    sleep 10
    waited=$(( waited + 10 ))
  done
}

target_of() {
  case "$1" in
    protocol | tui | agents | performance | network | input_features) echo "integration_test/m6_$1_test.dart" ;;
    *) echo "未知套件：$1（可选：protocol / tui / agents / performance / network / input_features）" >&2; exit 2 ;;
  esac
}

run() {
  local suite="$1" device="$2"
  shift 2
  local target udid report="${M6_REPORT:-${suite}-${device}}"
  target="$(target_of "${suite}")"
  udid="$(udid_of "${device}")"
  boot "${udid}"
  mkdir -p build/m6/logs
  local log="build/m6/logs/${report}.log"
  echo "== ${suite} @ ${device}（${udid}）→ ${log}"
  # 先构建一次（构建会把负载拉满），负载降下来再 drive（它只剩增量构建与安装）。
  flutter build ios --simulator --debug -t "${target}" \
    --dart-define=M6_DEVICE="${device}" "$@" >"${log}" 2>&1
  wait_for_load 2>&1 | tee -a "${log}"
  local status=0
  M6_REPORT="${report}" flutter drive \
    --driver=test_driver/integration_test.dart --target="${target}" -d "${udid}" \
    --dart-define=M6_DEVICE="${device}" "$@" >>"${log}" 2>&1 || status=$?
  grep -E 'All tests passed|Some tests failed|Failure in method' "${log}" | sed 's/^/   /' || true
  return "${status}"
}

case "${1:-}" in
  sims)
    for device in "${DEVICES[@]}"; do
      printf '%-18s %s\n' "${device}" "$(udid_of "${device}")"
    done
    ;;
  run)
    [ $# -ge 3 ] || usage
    shift
    run "$@"
    ;;
  matrix)
    shift
    suites=("$@")
    [ ${#suites[@]} -gt 0 ] || suites=("${SUITES[@]}")
    failed=0
    for device in "${DEVICES[@]}"; do
      for suite in "${suites[@]}"; do
        run "${suite}" "${device}" || failed=$(( failed + 1 ))
      done
      xcrun simctl shutdown "$(udid_of "${device}")" 2>/dev/null || true
    done
    echo "失败的组合：${failed}"
    [ "${failed}" -eq 0 ]
    ;;
  ui)
    [ $# -ge 2 ] || usage
    device="$2"
    udid="$(udid_of "${device}")"
    boot "${udid}"
    mkdir -p build/m6/logs
    log="build/m6/logs/ui-${device}.log"
    echo "== ui @ ${device}（${udid}）→ ${log}"
    # Runner scheme 的 Dart 入口取自最近一次 flutter build：先按 App 自己的入口构建一次。
    flutter build ios --simulator --debug >"${log}" 2>&1
    wait_for_load 2>&1 | tee -a "${log}"
    rm -rf "build/m6/ui-${device}.xcresult"
    status=0
    xcodebuild test -workspace ios/Runner.xcworkspace -scheme Runner \
      -destination "platform=iOS Simulator,id=${udid}" -only-testing:RunnerUITests \
      -resultBundlePath "build/m6/ui-${device}.xcresult" >>"${log}" 2>&1 || status=$?
    grep -E "Test Case .*(passed|failed)|error: -\[|XCTAssert|Executed" "${log}" | sed 's/^/   /' | tail -40 || true
    exit "${status}"
    ;;
  macos)
    [ $# -ge 2 ] || usage
    suite="$2"
    shift 2
    target="$(target_of "${suite}")"
    report="${M6_REPORT:-${suite}-macos}"
    mkdir -p build/m6/logs
    log="build/m6/logs/${report}.log"
    echo "== ${suite} @ macOS（profile）→ ${log}"
    flutter build macos --profile -t "${target}" --dart-define=M6_DEVICE=macos "$@" >"${log}" 2>&1
    wait_for_load 2>&1 | tee -a "${log}"
    M6_REPORT="${report}" flutter drive --profile -d macos \
      --driver=test_driver/integration_test.dart --target="${target}" \
      --dart-define=M6_DEVICE=macos "$@" >>"${log}" 2>&1 || status=$?
    grep -E 'All tests passed|Some tests failed|Failure in method' "${log}" | sed 's/^/   /' || true
    exit "${status:-0}"
    ;;
  device)
    [ $# -ge 3 ] || usage
    : "${M6_HOST:?真机测试需要 M6_HOST 指向设备可达的验收服务器}"
    suite="$2"
    udid="$3"
    shift 3
    target="$(target_of "${suite}")"
    device="${M6_DEVICE:-physical-device}"
    report="${M6_REPORT:-${suite}-${device}}"
    mode="${M6_MODE:-profile}"
    case "${mode}" in debug | profile) ;; *) echo "M6_MODE 只支持 debug 或 profile" >&2; exit 2 ;; esac
    mkdir -p build/m6/logs
    log="build/m6/logs/${report}.log"
    echo "== ${suite} @ 真机（${mode}）→ ${log}"
    status=0
    M6_REPORT="${report}" flutter drive "--${mode}" -d "${udid}" \
      --driver=test_driver/integration_test.dart --target="${target}" \
      --dart-define=M6_DEVICE="${device}" --dart-define=M6_HOST="${M6_HOST}" \
      --dart-define=M6_PORT="${M6_PORT:-2223}" \
      --dart-define=M6_LLM_HOST="${M6_LLM_HOST:-${M6_HOST}}" \
      --dart-define=M6_LLM_PORT="${M6_LLM_PORT:-2224}" \
      "$@" >"${log}" 2>&1 || status=$?
    # 构建失败时不能接受设备上旧 App 的通过结果。
    if grep -qE '^Failed to build iOS app|^Could not build the precompiled application' "${log}"; then
      echo "真机验收失败：当前测试 App 构建未通过" >&2
      status=1
    fi
    grep -E 'All tests passed|Some tests failed|Failure in method|Error|Exception' "${log}" | tail -30 || true
    exit "${status}"
    ;;
  ui-device)
    [ $# -ge 2 ] || usage
    : "${M6_HOST:?真机测试需要 M6_HOST 指向设备可达的验收服务器}"
    udid="$2"
    shift 2
    device="${M6_DEVICE:-physical-device}"
    report="${M6_REPORT:-ui-${device}}"
    mkdir -p build/m6/logs
    log="build/m6/logs/${report}.log"
    result="build/m6/${report}-$(date +%Y%m%d-%H%M%S).xcresult"
    echo "== UI @ 真机 → ${log} / ${result}"
    flutter build ios --profile >"${log}" 2>&1
    status=0
    TEST_RUNNER_M6_USE_FORM=1 TEST_RUNNER_M6_HOST="${M6_HOST}" TEST_RUNNER_M6_PORT="${M6_PORT:-2223}" \
      TEST_RUNNER_M6_LLM_HOST="${M6_LLM_HOST:-${M6_HOST}}" \
      TEST_RUNNER_M6_LLM_PORT="${M6_LLM_PORT:-2224}" \
      xcodebuild test -workspace ios/Runner.xcworkspace -scheme Runner -configuration Profile \
      -destination "platform=iOS,id=${udid}" "-only-testing:${M6_UI_TESTS:-RunnerUITests}" \
      -parallel-testing-enabled NO -collect-test-diagnostics never \
      -allowProvisioningUpdates -allowProvisioningDeviceRegistration \
      -resultBundlePath "${result}" "BUILD_DIR=${ROOT}/build/ios" "$@" >>"${log}" 2>&1 || status=$?
    grep -E "Test Case .*(passed|failed)|error: -\\[|XCTAssert|Executed|error:" "${log}" | tail -40 || true
    exit "${status}"
    ;;
  report)
    python3 "${HERE}/m6-report.py" build/m6/reports > build/m6/summary.md
    echo "build/m6/summary.md"
    ;;
  *)
    usage
    ;;
esac
