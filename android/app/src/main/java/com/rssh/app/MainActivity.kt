package com.rssh.app

import android.content.ClipDescription
import android.content.ClipboardManager
import android.os.Bundle
import android.util.Log
import android.view.ActionMode
import android.view.MenuItem
import android.view.View
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import org.json.JSONException
import org.json.JSONObject
import org.json.JSONTokener

class MainActivity : TauriActivity() {
    private var backNavigationEnabled = false
    private var imeVisible = false
    private var terminalWebView: WebView? = null
    private var terminalActionMode: ActionMode? = null
    private val terminalPasteMenuId = View.generateViewId()
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
            val caption = insets.getInsets(WindowInsetsCompat.Type.captionBar())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            imeVisible = insets.isVisible(WindowInsetsCompat.Type.ime())
            updateBackCallback()
            val bottom = maxOf(bars.bottom, ime.bottom)
            // The app's top navigation replaces the hidden status bar. Do not
            // reserve that row for the cutout, natively or via CSS safe-area.
            // Keep the window caption, side cutouts, bottom bar and IME clear.
            view.setPadding(bars.left, caption.top, bars.right, bottom)
            WindowInsetsCompat.CONSUMED
        }
        hideStatusBar()
    }

    override fun onWebViewCreate(webView: WebView) {
        super.onWebViewCreate(webView)
        terminalWebView = webView
        // Match the starting window until the document paints its saved theme.
        webView.setBackgroundColor(getColor(R.color.startup_background))
    }

    override fun onActionModeStarted(mode: ActionMode) {
        super.onActionModeStarted(mode)
        if (mode.type != ActionMode.TYPE_FLOATING) return
        terminalActionMode = mode
        val webView = terminalWebView ?: return
        webView.evaluateJavascript(
            """
            (() => {
                const selection = window.getSelection();
                if (!selection || selection.isCollapsed || !selection.toString()) return null;
                const element = node => node instanceof Element ? node : node?.parentElement;
                const anchor = element(selection.anchorNode);
                const focus = element(selection.focusNode);
                const host = anchor?.closest('.xterm-host[data-terminal-session]');
                if (!host || focus?.closest('.xterm-host[data-terminal-session]') !== host) return null;
                if (anchor.isContentEditable || focus.isContentEditable
                    || anchor.closest('input, textarea') || focus.closest('input, textarea')) return null;
                if (!host.isConnected || !host.getClientRects().length
                    || getComputedStyle(host).visibility !== 'visible') return null;
                return host.dataset.terminalSession || null;
            })()
            """.trimIndent()
        ) { result ->
            if (terminalActionMode !== mode || terminalWebView !== webView || isDestroyed) return@evaluateJavascript
            val sessionId = try {
                JSONTokener(result).nextValue() as? String
            } catch (error: JSONException) {
                Log.w("RsshPaste", "Unable to identify the selected terminal", error)
                null
            } ?: return@evaluateJavascript
            try {
                val clipboard = getSystemService(ClipboardManager::class.java)
                if (!isTextClipboard(clipboard.primaryClipDescription)) return@evaluateJavascript
                // Chromium refreshes its own menu groups. Keep this item in an
                // app-owned group so dragging the system handles preserves it.
                mode.menu.add(terminalPasteMenuId, terminalPasteMenuId, 0, android.R.string.paste)
                    .setShowAsActionFlags(MenuItem.SHOW_AS_ACTION_IF_ROOM)
                    .setOnMenuItemClickListener {
                        if (terminalActionMode === mode && terminalWebView === webView && !isDestroyed) {
                            pasteIntoTerminal(webView, mode, sessionId)
                        }
                        true
                    }
                mode.invalidateContentRect()
            } catch (error: Exception) {
                dispatchTerminalPaste(webView, mode, sessionId, "error", error.message ?: error.javaClass.simpleName)
            }
        }
    }

    override fun onActionModeFinished(mode: ActionMode) {
        super.onActionModeFinished(mode)
        if (terminalActionMode === mode) terminalActionMode = null
    }

    private fun isTextClipboard(description: ClipDescription?): Boolean =
        description?.hasMimeType(ClipDescription.MIMETYPE_TEXT_PLAIN) == true
            || description?.hasMimeType(ClipDescription.MIMETYPE_TEXT_HTML) == true

    private fun pasteIntoTerminal(webView: WebView, mode: ActionMode, sessionId: String) {
        try {
            // Read clipboard contents only after the user chooses Paste.
            val clip = getSystemService(ClipboardManager::class.java).primaryClip
            if (clip == null || clip.itemCount == 0 || !isTextClipboard(clip.description)) {
                mode.finish()
                return
            }
            val item = clip.getItemAt(0)
            val text = (item.text ?: item.coerceToText(this)).toString()
            dispatchTerminalPaste(webView, mode, sessionId, "text", text)
        } catch (error: Exception) {
            dispatchTerminalPaste(webView, mode, sessionId, "error", error.message ?: error.javaClass.simpleName)
        }
    }

    private fun dispatchTerminalPaste(
        webView: WebView, mode: ActionMode, sessionId: String, field: String, value: String
    ) {
        if (terminalActionMode !== mode || terminalWebView !== webView || isDestroyed) return
        webView.evaluateJavascript(
            """
            (() => {
                const sessionId = ${JSONObject.quote(sessionId)};
                const host = Array.from(document.querySelectorAll('.xterm-host[data-terminal-session]'))
                    .find(element => element.dataset.terminalSession === sessionId && element.isConnected
                        && element.getClientRects().length && getComputedStyle(element).visibility === 'visible');
                if (host) host.dispatchEvent(new CustomEvent('terminal:paste:' + sessionId,
                    { detail: { ${JSONObject.quote(field)}: ${JSONObject.quote(value)} } }));
            })()
            """.trimIndent()
        ) {
            // Finishing can collapse the selection. Route using the original
            // session, and never let an old callback close a newer action mode.
            if (terminalActionMode === mode) mode.finish()
        }
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
