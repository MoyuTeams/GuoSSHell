//! Android 宿主在 Flutter 引擎启动前提供应用上下文，供 Keystore 后端使用。

use jni::JNIEnv;
use jni::objects::JObject;

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_guosshell_NativeCredentials_00024Companion_initialize(
    env: JNIEnv,
    receiver: JObject,
    context: JObject,
) {
    android_native_keyring_store::Java_io_crates_keyring_Keyring_00024Companion_initializeNdkContext(
        env, receiver, context,
    );
}
