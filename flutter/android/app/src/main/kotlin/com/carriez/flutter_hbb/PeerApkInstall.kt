package com.carriez.flutter_hbb

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

private const val PEER_APK_LOG = "PeerApkInstall"
private const val PEER_APK_ACTION = "com.carriez.flutter_hbb.PEER_APK_UPDATE"

internal fun peerApkCertWire(context: Context, apkPath: String): String {
    val file = File(apkPath)
    apkFileCode(file.name, file.length(), symlink(file), file.isFile)?.let { return it }
    return try {
        val installedInfo = packageInfo(context, context.packageName, null)
        val archiveInfo = context.packageManager.getPackageArchiveInfo(apkPath, signingFlags())
        val installedHex = certSha256Hex(installedInfo).orEmpty()
        val candidateHex = certSha256Hex(archiveInfo).orEmpty()
        val plan = installPlan(
            installedHex,
            candidateHex,
            context.packageName,
            archiveInfo?.packageName.orEmpty(),
        )
        certWire(plan, installedHex, candidateHex)
    } catch (e: Exception) {
        Log.e(PEER_APK_LOG, "peer apk certs failed", e)
        UPDATE_SIGNATURE
    }
}

/**
 * Same-signature replacement of this package. Does not uninstall.
 * Never returns [UPDATE_OK]. A commit the OS accepts returns "committed"
 * and kills this process. The id result is reported after restart.
 */
internal fun peerApkCoverInstall(context: Context, apkPath: String): String {
    val file = File(apkPath)
    apkFileCode(file.name, file.length(), symlink(file), file.isFile)?.let { return it }
    val plan = try {
        val installedInfo = packageInfo(context, context.packageName, null)
        val archiveInfo = context.packageManager.getPackageArchiveInfo(apkPath, signingFlags())
        installPlan(
            certSha256Hex(installedInfo).orEmpty(),
            certSha256Hex(archiveInfo).orEmpty(),
            context.packageName,
            archiveInfo?.packageName.orEmpty(),
        )
    } catch (e: Exception) {
        Log.e(PEER_APK_LOG, "peer apk certs failed", e)
        return UPDATE_SIGNATURE
    }
    if (plan != UPDATE_OK) return plan
    return try {
        commitCover(context, file)
    } catch (e: Exception) {
        Log.e(PEER_APK_LOG, "peer apk install failed", e)
        UPDATE_INSTALL
    }
}

private fun commitCover(context: Context, file: File): String {
    val installer = context.packageManager.packageInstaller
    val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
    params.setAppPackageName(context.packageName)
    if (Build.VERSION.SDK_INT >= 31) {
        params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
    }
    val sessionId = installer.createSession(params)
    val session = installer.openSession(sessionId)
    try {
        session.openWrite("base.apk", 0, file.length()).use { out ->
            file.inputStream().use { input ->
                val buf = ByteArray(PEER_APK_CHUNK)
                var total = 0L
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    total += n
                    if (total > PEER_APK_MAX_BYTES || total > file.length()) {
                        session.abandon()
                        return UPDATE_CHECKSUM
                    }
                    out.write(buf, 0, n)
                }
                if (total != file.length()) {
                    session.abandon()
                    return UPDATE_CHECKSUM
                }
            }
            session.fsync(out)
        }
        return waitForSession(context, session, sessionId)
    } catch (e: Exception) {
        try {
            session.abandon()
        } catch (_: Exception) {
        }
        throw e
    } finally {
        try {
            session.close()
        } catch (_: Exception) {
        }
    }
}

private fun waitForSession(context: Context, session: PackageInstaller.Session, sessionId: Int): String {
    val slot = Slot()
    val action = PEER_APK_ACTION
    val receiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent) {
            val got = intent.getIntExtra(PackageInstaller.EXTRA_SESSION_ID, -1)
            if (got != -1 && got != sessionId) return
            val status = intent.getIntExtra(
                PackageInstaller.EXTRA_STATUS,
                PackageInstaller.STATUS_FAILURE,
            )
            when (status) {
                PackageInstaller.STATUS_SUCCESS -> {
                    // The process is replaced after this. Do not report ok here.
                    slot.code.set("committed")
                    slot.latch.countDown()
                }
                PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                    if (slot.prompted) return
                    slot.prompted = true
                    val confirm = confirmation(intent)
                    if (confirm == null) {
                        slot.code.set(UPDATE_INSTALL)
                        slot.latch.countDown()
                        return
                    }
                    confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    try {
                        ctx.startActivity(confirm)
                    } catch (e: Exception) {
                        Log.e(PEER_APK_LOG, "peer apk confirm intent failed", e)
                        slot.code.set(UPDATE_INSTALL)
                        slot.latch.countDown()
                    }
                }
                else -> {
                    val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
                    Log.e(PEER_APK_LOG, "peer apk installer status $status $message")
                    slot.code.set(UPDATE_INSTALL)
                    slot.latch.countDown()
                }
            }
        }
    }
    val filter = IntentFilter(action)
    try {
        if (Build.VERSION.SDK_INT >= 33) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            context.registerReceiver(receiver, filter)
        }
        val intent = Intent(action).setPackage(context.packageName)
        val flags = if (Build.VERSION.SDK_INT >= 31) {
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }
        val pending = PendingIntent.getBroadcast(context, sessionId, intent, flags)
        session.commit(pending.intentSender)
        val finished = slot.latch.await(PEER_APK_INSTALL_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        if (!finished) {
            try {
                session.abandon()
            } catch (_: Exception) {
            }
            return UPDATE_INSTALL
        }
        return slot.code.get()
    } finally {
        try {
            context.unregisterReceiver(receiver)
        } catch (_: Exception) {
        }
    }
}

private class Slot {
    val latch = CountDownLatch(1)
    val code = AtomicReference(UPDATE_INSTALL)
    var prompted = false
}

private fun confirmation(intent: Intent): Intent? {
    return if (Build.VERSION.SDK_INT >= 33) {
        intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
    } else {
        @Suppress("DEPRECATION")
        intent.getParcelableExtra(Intent.EXTRA_INTENT)
    }
}

private fun packageInfo(context: Context, packageName: String, path: String?): PackageInfo {
    val flags = signingFlags()
    if (path != null) {
        return context.packageManager.getPackageArchiveInfo(path, flags)
            ?: throw IllegalStateException("no archive info")
    }
    @Suppress("DEPRECATION")
    return context.packageManager.getPackageInfo(packageName, flags)
}

private fun signingFlags(): Int {
    return if (Build.VERSION.SDK_INT >= 28) {
        PackageManager.GET_SIGNING_CERTIFICATES
    } else {
        @Suppress("DEPRECATION")
        PackageManager.GET_SIGNATURES
    }
}

private fun certSha256Hex(info: PackageInfo?): String? {
    val signatures = signaturesOf(info) ?: return null
    if (signatures.isEmpty()) return null
    val digest = MessageDigest.getInstance("SHA-256").digest(signatures[signatures.size - 1].toByteArray())
    val out = StringBuilder(digest.size * 2)
    for (byte in digest) {
        out.append("%02x".format(byte))
    }
    return out.toString()
}

private fun signaturesOf(info: PackageInfo?): Array<android.content.pm.Signature>? {
    if (info == null) return null
    if (Build.VERSION.SDK_INT >= 28) {
        val signing = info.signingInfo ?: return null
        return if (signing.hasMultipleSigners()) {
            signing.apkContentsSigners
        } else {
            signing.signingCertificateHistory
        }
    }
    @Suppress("DEPRECATION")
    return info.signatures
}

private fun symlink(file: File): Boolean {
    if (Build.VERSION.SDK_INT < 26) return false
    return try {
        java.nio.file.Files.isSymbolicLink(file.toPath())
    } catch (_: Exception) {
        false
    }
}
