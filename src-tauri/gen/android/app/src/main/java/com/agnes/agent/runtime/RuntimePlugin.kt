package com.agnes.agent.runtime

import android.app.Activity
import android.content.Intent
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@TauriPlugin
class RuntimePlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun start(invoke: Invoke) {
    try {
      RuntimeService.markStartRequested()
      val intent = Intent(activity, RuntimeService::class.java)
      ContextCompat.startForegroundService(activity, intent)
      invoke.resolveObject(status())
    } catch (exception: Exception) {
      RuntimeService.markStopped()
      invoke.reject(exception.message ?: "Unable to start runtime service")
    }
  }

  @Command
  fun stop(invoke: Invoke) {
    try {
      activity.stopService(Intent(activity, RuntimeService::class.java))
      RuntimeService.markStopped()
      invoke.resolveObject(status())
    } catch (exception: Exception) {
      invoke.reject(exception.message ?: "Unable to stop runtime service")
    }
  }

  @Command
  fun getStatus(invoke: Invoke) {
    invoke.resolveObject(status())
  }

  private fun status(): JSObject {
    return JSObject().put("running", RuntimeService.isRunning())
  }
}
