package com.agnes.agent.runtime

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.system.Os
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileOutputStream

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

  @Command
  fun getRuntimeEnvironment(invoke: Invoke) {
    invoke.resolveObject(runtimeEnvironment())
  }

  @Command
  fun prepareBundledRuntime(invoke: Invoke) {
    val abi = runtimeAbi()
    val assetPath = bundledRuntimeAsset(abi)
    val proot = prootLibrary()
    if (!assetExists(assetPath)) {
      invoke.reject("Bundled Android Runtime is not available for $abi")
      return
    }
    if (!proot.isFile) {
      invoke.reject("Bundled PRoot library is not available for $abi")
      return
    }

    val destinationDir = File(activity.filesDir, "android-runtime-bundled/$abi")
    val destination = File(destinationDir, "runtime.zip")
    val temporary = File(destinationDir, "runtime.zip.tmp")
    try {
      if (!destinationDir.exists() && !destinationDir.mkdirs()) {
        throw IllegalStateException("Unable to create bundled Runtime directory")
      }
      temporary.delete()
      activity.assets.open(assetPath).use { input ->
        FileOutputStream(temporary).use { output ->
          input.copyTo(output)
          output.fd.sync()
        }
      }
      Os.rename(temporary.absolutePath, destination.absolutePath)
      invoke.resolveObject(
        JSObject()
          .put("abi", abi)
          .put("packagePath", destination.absolutePath)
          .put("packageSize", destination.length())
      )
    } catch (exception: Exception) {
      temporary.delete()
      invoke.reject(exception.message ?: "Unable to prepare bundled Android Runtime")
    }
  }

  private fun status(): JSObject {
    return JSObject().put("running", RuntimeService.isRunning())
  }

  private fun runtimeEnvironment(): JSObject {
    val abi = runtimeAbi()
    val proot = prootLibrary()
    return JSObject()
      .put("abi", abi)
      .put("bundledRuntimeAvailable", assetExists(bundledRuntimeAsset(abi)))
      .put("prootAvailable", proot.isFile)
      .put("prootPath", proot.absolutePath)
  }

  private fun runtimeAbi(): String {
    return Build.SUPPORTED_ABIS
      .firstOrNull { it == "arm64-v8a" || it == "x86_64" }
      ?: Build.SUPPORTED_ABIS.firstOrNull()
      ?: "unsupported"
  }

  private fun bundledRuntimeAsset(abi: String): String {
    return "agnes-runtime/$abi/runtime.zip"
  }

  private fun prootLibrary(): File {
    return File(activity.applicationInfo.nativeLibraryDir, "libagnes_proot.so")
  }

  private fun assetExists(path: String): Boolean {
    return try {
      activity.assets.open(path).use { }
      true
    } catch (_: Exception) {
      false
    }
  }
}
