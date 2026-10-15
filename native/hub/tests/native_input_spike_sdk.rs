// 独立测试编译新 module；不表示共享 lib.rs 已注册或实际 DLL 已含这些导出。
#[path = "../src/native_input_spike_sdk/mod.rs"]
mod native_input_spike_sdk;

#[test]
fn fixture_wait_surface_is_send_without_starting_runtime() {
    fn assert_send<T: Send>(_: T) {}
    assert_send(native_input_spike_sdk::run_fixture_only_until_shutdown());
}
