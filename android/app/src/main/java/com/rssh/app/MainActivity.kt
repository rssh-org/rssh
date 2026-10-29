package com.rssh.app

import android.os.Bundle
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

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
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            imeVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
            updateBackCallback()
            view.setPadding(bars.left, bars.top, bars.right, maxOf(bars.bottom, ime.bottom))
            WindowInsetsCompat.CONSUMED
        }
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
