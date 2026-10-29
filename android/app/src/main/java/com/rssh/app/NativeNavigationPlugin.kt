package com.rssh.app

import android.app.Activity
import android.app.Application
import android.os.Bundle
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.lang.ref.WeakReference

@InvokeArg
class BackNavigationArgs {
    var enabled: Boolean = false
    var revision: Long = 0
    lateinit var windowLabel: String
    lateinit var onBack: Channel
}

@TauriPlugin
class NativeNavigationPlugin(activity: Activity) : Plugin(activity) {
    private var currentActivity = WeakReference(activity as MainActivity)
    private var lastRevision: Long = -1

    init {
        // Tauri keeps plugins across Activity recreation. Bind each new host,
        // whose back callback starts disabled until its frontend is ready.
        activity.application.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
                if (activity is MainActivity) currentActivity = WeakReference(activity)
            }

            override fun onActivityDestroyed(activity: Activity) {
                if (currentActivity.get() === activity) currentActivity.clear()
            }

            override fun onActivityStarted(activity: Activity) {}
            override fun onActivityResumed(activity: Activity) {}
            override fun onActivityPaused(activity: Activity) {}
            override fun onActivityStopped(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
        })
    }

    @Command
    fun setEnabled(invoke: Invoke) {
        val args = invoke.parseArgs(BackNavigationArgs::class.java)
        // Native commands run on the UI thread. A delayed page-load reset must
        // not overwrite a newer state, including after Activity recreation.
        if (args.revision <= lastRevision) {
            invoke.resolve()
            return
        }
        lastRevision = args.revision
        val activity = currentActivity.get()
        if (activity == null || activity.isDestroyed) {
            if (args.enabled) invoke.reject("Native navigation activity is unavailable")
            else invoke.resolve()
            return
        }
        activity.setBackNavigation(args.enabled) {
            val event = JSObject()
            event.put("windowLabel", args.windowLabel)
            args.onBack.send(event)
        }
        invoke.resolve()
    }
}
