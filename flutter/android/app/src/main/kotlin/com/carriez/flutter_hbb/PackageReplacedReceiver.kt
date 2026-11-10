package com.carriez.flutter_hbb

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log

/**
 * Cover-install kills the process. The system delivers this after the new
 * package is in place so the service can compare the saved id and report it
 * when the controller reconnects.
 */
class PackageReplacedReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) {
            return
        }
        Log.i("PackageReplaced", "peer apk replaced; starting service")
        val start = Intent(context, MainService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.startForegroundService(start)
        } else {
            context.startService(start)
        }
    }
}
