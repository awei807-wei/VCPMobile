package com.vcp.avatar

import android.app.Application
import com.vcp.mobile.CrashDiagnostics

/** 在 Tauri Activity 和原生插件初始化前安装进程级崩溃诊断。 */
class VcpApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        CrashDiagnostics.install(this)
    }
}
