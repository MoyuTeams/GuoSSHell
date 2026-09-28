package com.guosshell

import android.Manifest
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import com.guosshell.guosh_shell.MainActivity
import com.guosshell.guosh_shell.R

// 有 SSH 会话连着时的前台服务：App 切到后台后进程不被冻结，连接得以保持。
// 通知只显示会话数，点一下回到 App；会话都结束、或用户从最近任务里划掉 App 时停止。
class SessionKeeperService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val count = intent?.getIntExtra(EXTRA_COUNT, 0) ?: 0
        if (count <= 0) {
            stopSelf()
            return START_NOT_STICKY
        }
        val notification = notification(count)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        return START_NOT_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    private fun notification(count: Int): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "SSH 会话", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "App 在后台时保持 SSH 会话连接"
                    setShowBadge(false)
                },
            )
            Notification.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this).setPriority(Notification.PRIORITY_LOW)
        }
        return builder
            .setSmallIcon(R.drawable.ic_launcher_monochrome)
            .setContentTitle("GuoSSHell")
            .setContentText("$count 个 SSH 会话保持连接")
            .setContentIntent(open)
            .setOngoing(true)
            .setShowWhen(false)
            .build()
    }

    companion object {
        private const val TAG = "SessionKeeper"
        private const val CHANNEL_ID = "ssh_sessions"
        private const val NOTIFICATION_ID = 1
        private const val EXTRA_COUNT = "count"
        private const val NOTIFICATION_PERMISSION_REQUEST = 7301
        private const val PREFS = "session_keeper"
        private const val ASKED_NOTIFICATIONS = "asked_notifications"

        // 会话数变化（Dart 经 MethodChannel 通知）：有会话就开或更新前台服务，没有就停掉。
        fun update(activity: Activity, count: Int) {
            val intent = Intent(activity, SessionKeeperService::class.java)
            if (count <= 0) {
                activity.stopService(intent)
                return
            }
            askNotificationPermissionOnce(activity)
            intent.putExtra(EXTRA_COUNT, count)
            try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    activity.startForegroundService(intent)
                } else {
                    activity.startService(intent)
                }
            } catch (error: RuntimeException) {
                // App 已在后台时系统不允许启动前台服务（Android 12 起），回到前台后下一次变化会补上。
                Log.w(TAG, "前台服务未能启动", error)
            }
        }

        // 通知权限（Android 13 起）只影响通知是否显示，前台服务照常运行；只问一次。
        private fun askNotificationPermissionOnce(activity: Activity) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
            if (activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
                PackageManager.PERMISSION_GRANTED
            ) {
                return
            }
            val prefs = activity.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            if (prefs.getBoolean(ASKED_NOTIFICATIONS, false)) return
            prefs.edit().putBoolean(ASKED_NOTIFICATIONS, true).apply()
            activity.requestPermissions(
                arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                NOTIFICATION_PERMISSION_REQUEST,
            )
        }
    }
}
