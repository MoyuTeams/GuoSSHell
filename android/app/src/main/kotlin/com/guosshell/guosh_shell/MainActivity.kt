package com.guosshell.guosh_shell

import android.os.Bundle
import com.guosshell.NativeCredentials
import com.guosshell.SessionKeeperService
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        NativeCredentials.initialize(applicationContext)
        super.onCreate(savedInstanceState)
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // 会话保活：Dart 在需要保活的会话数变化时通知（lib/src/lifecycle/session_keeper.dart）。
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "guosshell/session_keeper")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "update" -> {
                        SessionKeeperService.update(this, call.arguments as? Int ?: 0)
                        result.success(null)
                    }
                    else -> result.notImplemented()
                }
            }
    }

    override fun onDestroy() {
        // Activity 真正结束（不是旋转等配置变化重建）时会话随 Flutter 引擎一起结束。
        if (isFinishing) SessionKeeperService.update(this, 0)
        super.onDestroy()
    }
}
