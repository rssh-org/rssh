package com.rssh.app

import android.os.Bundle
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
    private var backNavigationEnabled = false
    private var imeVisible = false
    private lateinit var onBackRequested: () -> Unit
    private val backCallback = object : OnBackPressedCallback(false) {
        override fun handleOnBackPressed() {
            onBackRequested()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        onBackPressedDispatcher.addCallback(this, backCallback)

        val content = findViewById<android.view.View>(android.R.id.content)
        ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
            )
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            imeVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
            updateBackCallback()
            val bottom = maxOf(bars.bottom, ime.bottom)
            // The app's top navigation replaces the hidden status bar. Do not
            // reserve that row for the cutout, natively or via CSS safe-area.
            // Keep side cutouts, the bottom navigation bar and the IME clear.
            view.setPadding(bars.left, 0, bars.right, bottom)
            WindowInsetsCompat.CONSUMED
        }
        hideStatusBar()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) hideStatusBar()
    }

    private fun hideStatusBar() {
        val controller = WindowCompat.getInsetsController(window, window.decorView)
        controller.systemBarsBehavior =
            WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller.hide(WindowInsetsCompat.Type.statusBars())
    }

    fun setBackNavigation(enabled: Boolean, onBack: () -> Unit) {
        onBackRequested = onBack
        backNavigationEnabled = enabled
        updateBackCallback()
    }

    private fun updateBackCallback() {
        // Let the system dismiss the IME before navigating within the app.
        backCallback.isEnabled = backNavigationEnabled && !imeVisible
    }
}
