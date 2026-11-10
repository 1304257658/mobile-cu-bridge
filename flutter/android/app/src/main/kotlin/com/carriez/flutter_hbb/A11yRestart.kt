package com.carriez.flutter_hbb

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.SystemClock
import android.util.Log

const val ACT_A11Y_RESTART = "com.carriez.flutter_hbb.A11Y_RESTART"

private const val A11Y_RESTART_REQUEST = 71
private const val A11Y_RESTART_DELAY_MS = 1_000L
private const val A11Y_RESTART_TAG = "a11y restart"

/**
 * Schedule MainService one second out, same package. An inexact alarm is not
 * used: it can miss the 60s bound and it is not the exact-alarm exemption for
 * starting a foreground service from the background.
 */
internal fun scheduleA11yRestart(context: Context): Boolean {
    val alarm = context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager
    if (alarm == null) {
        Log.e(A11Y_RESTART_TAG, "alarm service missing")
        return false
    }
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && !alarm.canScheduleExactAlarms()) {
        Log.e(A11Y_RESTART_TAG, "exact alarm not granted")
        return false
    }
    val intent = Intent(context, MainService::class.java).apply {
        action = ACT_A11Y_RESTART
    }
    val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
    } else {
        PendingIntent.FLAG_UPDATE_CURRENT
    }
    val pending = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        PendingIntent.getForegroundService(context, A11Y_RESTART_REQUEST, intent, flags)
    } else {
        PendingIntent.getService(context, A11Y_RESTART_REQUEST, intent, flags)
    }
    val at = SystemClock.elapsedRealtime() + A11Y_RESTART_DELAY_MS
    return try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            alarm.setExactAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pending)
        } else {
            alarm.setExact(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pending)
        }
        true
    } catch (e: SecurityException) {
        Log.e(A11Y_RESTART_TAG, "exact alarm refused", e)
        false
    }
}
