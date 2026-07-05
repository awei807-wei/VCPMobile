package com.vcp.mobile

import android.app.Activity
import android.content.res.Configuration
import android.webkit.WebView
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import app.tauri.plugin.JSObject
import java.lang.ref.WeakReference

/**
 * 应用生命周期桥接器
 *
 * 合并方案：保留本地回调 (onResumeHook/onConfigurationChangedHook) + 添加上游 ProcessLifecycleOwner 支持。
 * 
 * 通过 DefaultLifecycleObserver 自动监听进程级生命周期 (ProcessLifecycleOwner)，
 * 完美防抖，免疫 Activity 重建与切换。
 * 
 * 使用 plugin.trigger 派发强类型的原生生命周期事件，规避 WebView 被冻结时 JS 无法执行的痛点。
 * 同时保留本地的 onResumeHook/onConfigurationChangedHook 回调，用于权限刷新等本地逻辑。
 */
class LifecycleBridge(
    private val onResumeHook: (() -> Unit)? = null,
    private val onConfigurationChangedHook: ((Configuration) -> Unit)? = null
) : DefaultLifecycleObserver {

    private var activityRef: WeakReference<Activity>? = null
    private var pluginRef: WeakReference<VcpMobilePlugin>? = null

    fun attach(activity: Activity, plugin: VcpMobilePlugin) {
        activityRef = WeakReference(activity)
        pluginRef = WeakReference(plugin)
        // 升级为进程级生命周期监听，完美防抖，免疫 Activity 重建与切换
        activity.runOnUiThread {
            androidx.lifecycle.ProcessLifecycleOwner.get().lifecycle.addObserver(this)
        }
    }

    fun detach() {
        val activity = activityRef?.get()
        if (activity != null) {
            activity.runOnUiThread {
                try {
                    androidx.lifecycle.ProcessLifecycleOwner.get().lifecycle.removeObserver(this)
                } catch (_: Exception) {}
            }
        } else {
            try {
                androidx.lifecycle.ProcessLifecycleOwner.get().lifecycle.removeObserver(this)
            } catch (_: Exception) {}
        }
        activityRef = null
        pluginRef = null
    }

    override fun onDestroy(owner: LifecycleOwner) {
        detach()
        super.onDestroy(owner)
    }

    override fun onResume(owner: LifecycleOwner) {
        emit(mapOf("state" to "resume"))
        onResumeHook?.invoke()
    }

    override fun onPause(owner: LifecycleOwner) {
        emit(mapOf("state" to "pause"))
    }

    override fun onStop(owner: LifecycleOwner) {
        emit(mapOf("state" to "stop"))
    }

    fun onConfigurationChanged(newConfig: Configuration) {
        val uiMode = newConfig.uiMode and Configuration.UI_MODE_NIGHT_MASK
        val isDark = uiMode == Configuration.UI_MODE_NIGHT_YES
        emit(mapOf(
            "state" to "config-changed",
            "isDarkMode" to isDark
        ))
        onConfigurationChangedHook?.invoke(newConfig)
    }

    fun onLowMemory() {
        emit(mapOf("state" to "low-memory"))
    }

    private fun emit(detail: Map<String, Any?>) {
        // 向 Rust 侧派发强类型的原生生命周期事件，规避 WebView 被冻结时 JS 无法执行的痛点
        val plugin = pluginRef?.get()
        if (plugin != null) {
            val triggerData = JSObject()
            for ((key, value) in detail) {
                when (value) {
                    is String -> triggerData.put(key, value)
                    is Boolean -> triggerData.put(key, value)
                    is Int -> triggerData.put(key, value)
                    is Double -> triggerData.put(key, value)
                    is Long -> triggerData.put(key, value)
                }
            }
            plugin.trigger("lifecycle", triggerData)
        }
    }
}
