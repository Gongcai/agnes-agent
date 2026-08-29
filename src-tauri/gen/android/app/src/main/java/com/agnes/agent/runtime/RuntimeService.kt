package com.agnes.agent.runtime

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import com.agnes.agent.R

class RuntimeService : Service() {
  override fun onCreate() {
    super.onCreate()
    createNotificationChannel()
    isRunningFlag = true
    startForegroundCompat()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    isRunningFlag = true
    return START_STICKY
  }

  override fun onDestroy() {
    markStopped()
    stopForeground(STOP_FOREGROUND_REMOVE)
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null

  private fun startForegroundCompat() {
    val notification = buildNotification()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      startForeground(
        NOTIFICATION_ID,
        notification,
        ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
      )
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
  }

  private fun buildNotification(): Notification {
    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(R.mipmap.ic_launcher)
      .setContentTitle("Agnes Linux Runtime")
      .setContentText("Runtime service is running")
      .setOngoing(true)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .build()
  }

  private fun createNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    manager.createNotificationChannel(
      NotificationChannel(
        CHANNEL_ID,
        "Agnes Runtime",
        NotificationManager.IMPORTANCE_LOW,
      ),
    )
  }

  companion object {
    private const val CHANNEL_ID = "agnes-runtime"
    private const val NOTIFICATION_ID = 2001

    @Volatile
    private var isRunningFlag = false

    fun isRunning(): Boolean = isRunningFlag

    fun markStartRequested() {
      isRunningFlag = true
    }

    fun markStopped() {
      isRunningFlag = false
    }
  }
}
