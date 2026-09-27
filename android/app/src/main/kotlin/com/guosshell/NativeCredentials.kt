package com.guosshell

import android.content.Context

// Rust 的系统凭证后端需要在 Flutter 启动前取得应用上下文。
class NativeCredentials {
    companion object {
        init { System.loadLibrary("hub") }
        external fun initialize(context: Context)
    }
}
