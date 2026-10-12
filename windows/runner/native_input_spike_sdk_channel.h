#pragma once

#include <flutter/binary_messenger.h>
#include <flutter/encodable_value.h>
#include <flutter/method_channel.h>

#include <memory>

#include "native_input_spike_sdk/bootstrap_host.h"
#include "native_input_spike_live_source/source_fixture_host.h"

// 仅显式 fixture 进程持有；source 另须双开关及实际 SDK Verify。
class NativeInputSdkChannel final {
 public:
  explicit NativeInputSdkChannel(flutter::BinaryMessenger* messenger,
                                 HWND parent, bool source_enabled = false,
                                 bool source_exit_test_enabled = false);
  ~NativeInputSdkChannel();
  NativeInputSdkChannel(const NativeInputSdkChannel&) = delete;
  NativeInputSdkChannel& operator=(const NativeInputSdkChannel&) = delete;
  guosshell::native_input_spike::LoopDecision BeforeTranslate(const MSG&) noexcept;
  HRESULT CloseForWindow() noexcept;
  HRESULT PrepareForProcessExit() noexcept;
  static void PollRetainedSource() noexcept;
  static HRESULT ReportRetainedAtExit() noexcept;

 private:
  void Handle(const flutter::MethodCall<flutter::EncodableValue>& call,
              std::unique_ptr<flutter::MethodResult<flutter::EncodableValue>> result);
  guosshell::native_input_spike_live_source::Numbers InspectSourceNumbers() noexcept;

  guosshell::native_input_spike_sdk::BootstrapHost host_;
  const HWND parent_;
  const bool source_enabled_;
  const bool source_exit_test_enabled_;
  std::unique_ptr<guosshell::native_input_spike_live_source::SourceFixtureHost> source_;
  bool exit_prepared_ = false;
  bool exit_retained_ = false;
  HRESULT exit_status_ = S_OK;
  guosshell::native_input_spike_live_source::Numbers exit_numbers_{};
  std::uint32_t loop_handled_ = 0;
  std::uint32_t loop_dispatch_only_ = 0;
  std::uint32_t loop_stop_ = 0;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
};
