package dev.tunnelrat.drive

import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.io.FileNotFoundException

/** Turns one `FsOp` JSON object into an `FsResult` JSON object. Never throws. */
fun handleFsOp(fs: PhoneFs, op: JSONObject): JSONObject = try {
    fun p(key: String = "path") = op.getString(key)
    fun stat(s: FsStat) = JSONObject().put("dir", s.dir).put("size", s.size).put("mtime_ms", s.mtimeMs)
    val done = JSONObject().put("kind", "done")
    when (op.getString("op")) {
        "stat" -> JSONObject().put("kind", "stat").put("stat", stat(fs.stat(p())))
        "list" -> JSONObject().put("kind", "list").put(
            "entries",
            JSONArray().apply { fs.list(p()).forEach { (name, s) -> put(JSONObject().put("name", name).put("stat", stat(s))) } },
        )
        "read" -> {
            val len = op.getInt("len").coerceIn(0, 1024 * 1024) // tura_proto::FS_CHUNK
            val data = fs.read(p(), op.getLong("offset"), len)
            JSONObject().put("kind", "data").put("data", Base64.encodeToString(data, Base64.NO_WRAP))
        }
        "write" -> { fs.write(p(), op.getLong("offset"), Base64.decode(op.getString("data"), Base64.DEFAULT)); done }
        "create" -> { fs.create(p()); done }
        "mkdir" -> { fs.mkdir(p()); done }
        "remove" -> { fs.remove(p()); done }
        "rmdir" -> { fs.rmdir(p()); done }
        "rename" -> { fs.rename(p("from"), p("to")); done }
        "truncate" -> { fs.truncate(p(), op.getLong("size")); done }
        "stat_fs" -> fs.statFs().let { (total, free) -> JSONObject().put("kind", "stat_fs").put("total", total).put("free", free) }
        else -> error("io")
    }
} catch (e: FsException) {
    err(e.kind)
} catch (e: FileNotFoundException) {
    err("not_found")
} catch (e: SecurityException) {
    err("denied")
} catch (e: Exception) {
    android.util.Log.w("Tunnelrat", "fs op failed: ${op.optString("op")}: $e")
    err(if (e.message?.contains("ENOSPC") == true) "no_space" else "io")
}

private fun err(kind: String) = JSONObject().put("kind", "error").put("error", kind)

fun fsError(kind: String) = err(kind)

/** Reads for a binary reply; failures carry the PC's error kind. */
fun readOrError(fs: PhoneFs, op: JSONObject): Result<ByteArray> = runCatching {
    try {
        fs.read(op.getString("path"), op.getLong("offset"), op.getInt("len").coerceIn(0, 1024 * 1024))
    } catch (e: FileNotFoundException) {
        throw FsException.NotFound
    } catch (e: SecurityException) {
        throw FsException.Denied
    }
}

fun writeOrError(fs: PhoneFs, w: Bin.Write): JSONObject = try {
    fs.write(w.path, w.offset, w.data)
    JSONObject().put("kind", "done")
} catch (e: FsException) {
    err(e.kind)
} catch (e: FileNotFoundException) {
    err("not_found")
} catch (e: SecurityException) {
    err("denied")
} catch (e: Exception) {
    err(if (e.message?.contains("ENOSPC") == true) "no_space" else "io")
}

/** Binary drive frames; layout documented in `tura_proto::bin`. */
object Bin {
    const val READ_DATA: Byte = 1
    const val WRITE: Byte = 2

    class Write(val id: Long, val path: String, val offset: Long, val data: ByteArray)

    fun readData(id: Long, data: ByteArray): ByteArray =
        java.nio.ByteBuffer.allocate(9 + data.size).put(READ_DATA).putLong(id).put(data).array()

    fun parseWrite(frame: ByteArray): Write? = runCatching {
        val b = java.nio.ByteBuffer.wrap(frame) // big-endian by default
        if (b.get() != WRITE) return null
        val id = b.getLong()
        val path = ByteArray(b.getShort().toInt() and 0xFFFF).also { b.get(it) }.toString(Charsets.UTF_8)
        val offset = b.getLong()
        Write(id, path, offset, ByteArray(b.remaining()).also { b.get(it) })
    }.getOrNull()
}
