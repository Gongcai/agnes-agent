package com.agnes.agent.runtime

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.os.Build
import android.system.Os
import android.util.Log
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileOutputStream
import org.json.JSONArray

@TauriPlugin
class RuntimePlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun start(invoke: Invoke) {
    try {
      RuntimeService.markStartRequested()
      val intent = Intent(activity, RuntimeService::class.java)
      ContextCompat.startForegroundService(activity, intent)
      invoke.resolve(status())
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
      invoke.resolve(status())
    } catch (exception: Exception) {
      invoke.reject(exception.message ?: "Unable to stop runtime service")
    }
  }

  @Command
  fun getStatus(invoke: Invoke) {
    invoke.resolve(status())
  }

  @Command
  fun getRuntimeEnvironment(invoke: Invoke) {
    invoke.resolve(runtimeEnvironment())
  }

  @Command
  fun prepareBundledRuntime(invoke: Invoke) {
    val abi = runtimeAbi()
    val assetPath = bundledRuntimeAsset(abi)
    val proot = prootLibrary()
    val prootLoader = prootLoader()
    if (!assetExists(assetPath)) {
      invoke.reject("Bundled Android Runtime is not available for $abi")
      return
    }
    if (!proot.isFile) {
      invoke.reject("Bundled PRoot library is not available for $abi")
      return
    }
    if (!prootLoader.isFile) {
      invoke.reject("Bundled PRoot loader is not available for $abi")
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
      val result = JSObject()
      result.put("abi", abi)
      result.put("packagePath", destination.absolutePath)
      result.put("packageSize", destination.length())
      invoke.resolve(result)
    } catch (exception: Exception) {
      temporary.delete()
      invoke.reject(exception.message ?: "Unable to prepare bundled Android Runtime")
    }
  }

  private fun status(): JSObject {
    val result = JSObject()
    result.put("running", RuntimeService.isRunning())
    return result
  }

  private fun runtimeEnvironment(): JSObject {
    val abi = runtimeAbi()
    val proot = prootLibrary()
    val prootLoader = prootLoader()
    val bundledRuntimeAvailable = assetExists(bundledRuntimeAsset(abi))
    Log.i(
      "AgnesRuntime",
      "environment abi=$abi bundled=$bundledRuntimeAvailable proot=${proot.isFile} loader=${prootLoader.isFile}"
    )
    val result = JSObject()
    result.put("abi", abi)
    result.put("bundledRuntimeAvailable", bundledRuntimeAvailable)
    result.put("prootAvailable", proot.isFile)
    result.put("prootPath", proot.absolutePath)
    result.put("prootLoaderAvailable", prootLoader.isFile)
    result.put("prootLoaderPath", prootLoader.absolutePath)
    val dns = JSONArray()
    dnsServers().forEach { dns.put(it) }
    result.put("dnsServers", dns)
    return result
  }

  private fun dnsServers(): List<String> {
    val connectivity = activity.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    val networks = buildList {
      connectivity.activeNetwork?.let(::add)
      addAll(connectivity.allNetworks.asList())
    }
    return networks
      .distinct()
      .asSequence()
      .mapNotNull { connectivity.getLinkProperties(it) }
      .flatMap { it.dnsServers.asSequence() }
      .mapNotNull { it.hostAddress?.substringBefore('%') }
      .filter { it.isNotBlank() }
      .distinct()
      .toList()
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

  private fun prootLoader(): File {
    return File(activity.applicationInfo.nativeLibraryDir, "libagnes_proot_loader.so")
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
