package com.vcp.mobile

import android.app.Activity
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin

@TauriPlugin(permissions = [
    Permission(strings = ["android.permission.POST_NOTIFICATIONS"], alias = "notification"),
    Permission(strings = ["android.permission.READ_MEDIA_IMAGES"], alias = "storage"),
    Permission(strings = ["android.permission.READ_EXTERNAL_STORAGE", "android.permission.WRITE_EXTERNAL_STORAGE"], alias = "storageLegacy"),
    Permission(strings = ["android.permission.RECORD_AUDIO"], alias = "microphone"),
    Permission(strings = ["android.permission.CAMERA"], alias = "camera"),
    Permission(strings = ["android.permission.ACCESS_FINE_LOCATION", "android.permission.ACCESS_COARSE_LOCATION"], alias = "location")
])
class VcpMobilePlugin(activity: Activity) : VcpMobileNotificationCommands(activity) {
    companion object {
        const val TAG = "VcpMobilePlugin"
        private var instanceRef: java.lang.ref.WeakReference<VcpMobilePlugin>? = null

        fun getInstance(): VcpMobilePlugin? {
            return instanceRef?.get()
        }
    }

    init {
        instanceRef = java.lang.ref.WeakReference(this)
    }
}
