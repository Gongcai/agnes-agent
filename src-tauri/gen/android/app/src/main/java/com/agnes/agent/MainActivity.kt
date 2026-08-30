package com.agnes.agent

import android.os.Bundle
import android.graphics.Color
import android.view.View
import android.view.WindowManager
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  // Let the WebView consume Android back presses before the activity closes.
  override val handleBackNavigation: Boolean = true

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)

    // Keep the webview below Android system bars while retaining edge-to-edge compatibility.
    window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
    WindowCompat.setDecorFitsSystemWindows(window, false)
    window.statusBarColor = Color.rgb(245, 240, 232)
    window.navigationBarColor = Color.rgb(250, 249, 245)
    WindowCompat.getInsetsController(window, window.decorView).apply {
      isAppearanceLightStatusBars = true
      isAppearanceLightNavigationBars = true
    }

    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      insets
    }
    ViewCompat.requestApplyInsets(content)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)

    // Some Android WebViews keep the layout viewport at full height when the
    // IME opens. Pass the native IME inset to the web UI so fixed composers can
    // be positioned above the keyboard without relying on visualViewport.
    ViewCompat.setOnApplyWindowInsetsListener(webView) { _, insets ->
      val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
      val imeHeight = if (insets.isVisible(WindowInsetsCompat.Type.ime())) {
        // The content view already applies the navigation bar inset as bottom
        // padding, so only expose the IME overlap inside the WebView viewport.
        maxOf(0, ime.bottom - bars.bottom)
      } else {
        0
      }
      val density = resources.displayMetrics.density
      val cssHeight = imeHeight / density
      val imeVisible = imeHeight > 0
      webView.post {
        webView.evaluateJavascript(
          "document.documentElement.style.setProperty('--agnes-native-ime-height', '${cssHeight}px'); window.dispatchEvent(new CustomEvent('agnes:native-ime-change', { detail: { visible: $imeVisible, height: $cssHeight } }))",
          null,
        )
      }
      insets
    }
    ViewCompat.requestApplyInsets(webView)
  }
}
