package com.guosshell.guosh_shell

import android.os.Bundle
import com.guosshell.NativeCredentials
import io.flutter.embedding.android.FlutterActivity

class MainActivity : FlutterActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        NativeCredentials.initialize(applicationContext)
        super.onCreate(savedInstanceState)
    }
}
