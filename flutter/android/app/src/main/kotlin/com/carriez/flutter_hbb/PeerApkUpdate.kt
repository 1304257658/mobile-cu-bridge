package com.carriez.flutter_hbb

/**
 * Decisions for a same-signature cover-install of this peer APK.
 * No Android types, so the unit test can run without an SDK.
 *
 * Success is only [UPDATE_OK] when the signing cert matches and the
 * device id / enc_id are unchanged. There is no uninstall plan.
 */

internal const val UPDATE_OK = "ok"
internal const val UPDATE_NETWORK = "network"
internal const val UPDATE_CHECKSUM = "checksum"
internal const val UPDATE_SIGNATURE = "signature"
internal const val UPDATE_INSTALL = "install"
internal const val UPDATE_ID_CHANGED = "id_changed"

internal const val PEER_APK_NAME = "rustdesk-peer-update.apk"
internal const val PEER_APK_MAX_BYTES = 96L * 1024L * 1024L
internal const val PEER_APK_CHUNK = 8192
internal const val PEER_APK_INSTALL_TIMEOUT_MS = 40_000L
internal const val CERT_SHA256_HEX = 64

internal fun certHexOk(hex: String): Boolean {
    if (hex.length != CERT_SHA256_HEX) return false
    for (ch in hex) {
        val ok = ch in '0'..'9' || ch in 'a'..'f' || ch in 'A'..'F'
        if (!ok) return false
    }
    return true
}

/** `ok` means cover-install this package. Anything else must not install. */
internal fun installPlan(
    installedHex: String,
    candidateHex: String,
    packageName: String,
    archivePackage: String,
): String {
    if (packageName.isEmpty() || packageName != archivePackage) return UPDATE_SIGNATURE
    if (!certHexOk(installedHex) || !certHexOk(candidateHex)) return UPDATE_SIGNATURE
    if (!installedHex.equals(candidateHex, ignoreCase = true)) return UPDATE_SIGNATURE
    return UPDATE_OK
}

internal fun certWire(plan: String, installedHex: String, candidateHex: String): String {
    if (plan != UPDATE_OK) return plan
    if (!certHexOk(installedHex) || !certHexOk(candidateHex)) return UPDATE_SIGNATURE
    return "ok\n${installedHex.lowercase()}\n${candidateHex.lowercase()}"
}

/**
 * `null` means the file may be streamed. A code means do not install.
 * A symlink or a file past [PEER_APK_MAX_BYTES] is [UPDATE_INSTALL].
 */
internal fun apkFileCode(name: String, size: Long, symlink: Boolean, isFile: Boolean): String? {
    if (symlink || !isFile) return UPDATE_INSTALL
    if (name != PEER_APK_NAME || size <= 0L || size > PEER_APK_MAX_BYTES) return UPDATE_INSTALL
    return null
}

/**
 * PASS is [UPDATE_OK] only when the stored id material is unchanged.
 * A missing device id, or an `enc_id` that moved, is [UPDATE_ID_CHANGED].
 */
internal fun idPreserved(
    deviceId: String,
    beforeEnc: String,
    afterEnc: String,
    beforePlain: String,
    afterPlain: String,
): String {
    if (deviceId.isEmpty() || deviceId.length > 64) return UPDATE_ID_CHANGED
    if (beforeEnc.isNotEmpty()) {
        return if (beforeEnc == afterEnc) UPDATE_OK else UPDATE_ID_CHANGED
    }
    return if (beforePlain == deviceId && afterPlain == deviceId) UPDATE_OK else UPDATE_ID_CHANGED
}
