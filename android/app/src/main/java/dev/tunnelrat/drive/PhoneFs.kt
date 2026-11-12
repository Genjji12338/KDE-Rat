package dev.tunnelrat.drive

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.os.StatFs
import android.provider.DocumentsContract
import android.provider.DocumentsContract.Document
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.RandomAccessFile

/** A file-system error the PC understands (`tura_proto::FsError`). */
class FsException(val kind: String) : Exception(kind) {
    companion object {
        val NotFound get() = FsException("not_found")
        val Exists get() = FsException("exists")
        val NotEmpty get() = FsException("not_empty")
        val NotDir get() = FsException("not_dir")
        val IsDir get() = FsException("is_dir")
        val Denied get() = FsException("denied")
        val Io get() = FsException("io")
    }
}

data class FsStat(val dir: Boolean, val size: Long, val mtimeMs: Long)

/**
 * What the PC can do with the shared files. Paths are `/`-separated and
 * relative to the shared root ("" = root). Implementations must never let a
 * path reach outside the root.
 */
interface PhoneFs {
    fun stat(path: String): FsStat
    fun list(path: String): List<Pair<String, FsStat>>
    fun read(path: String, offset: Long, len: Int): ByteArray
    fun write(path: String, offset: Long, data: ByteArray)
    fun create(path: String)
    fun mkdir(path: String)
    fun remove(path: String)
    fun rmdir(path: String)
    fun rename(from: String, to: String)
    fun truncate(path: String, size: Long)
    /** (total, free) bytes. */
    fun statFs(): Pair<Long, Long>
}

/** Rejects anything but plain names: no empty parts, ".", "..", or backslashes. */
internal fun parts(path: String): List<String> {
    if (path.isEmpty()) return emptyList()
    val p = path.split('/')
    if (p.any { it.isEmpty() || it == "." || it == ".." || '\\' in it || '\u0000' in it }) throw FsException.Denied
    return p
}

private fun statFsOf(dir: File): Pair<Long, Long> = StatFs(dir.path).let { it.totalBytes to it.availableBytes }

// ── Whole phone: plain files under shared storage ────────────────────────────

/**
 * Direct file access under [root] (needs "All files access"). Every path is
 * canonicalised (resolving symlinks) and must stay inside the root.
 */
class FileFs(root: File) : PhoneFs {
    private val root: File = root.canonicalFile
    private val rootPrefix = this.root.path + File.separator

    private fun f(path: String): File {
        val file = File(root, parts(path).joinToString(File.separator)).canonicalFile
        if (file != root && !file.path.startsWith(rootPrefix)) throw FsException.Denied
        return file
    }

    private fun File.st() = FsStat(isDirectory, if (isDirectory) 0 else length(), lastModified())

    override fun stat(path: String): FsStat = f(path).also { if (!it.exists()) throw FsException.NotFound }.st()

    override fun list(path: String): List<Pair<String, FsStat>> {
        val d = f(path)
        if (!d.exists()) throw FsException.NotFound
        if (!d.isDirectory) throw FsException.NotDir
        return (d.listFiles() ?: throw FsException.Denied).map { it.name to it.st() }
    }

    override fun read(path: String, offset: Long, len: Int): ByteArray {
        val file = f(path)
        if (file.isDirectory) throw FsException.IsDir
        if (!file.exists()) throw FsException.NotFound
        RandomAccessFile(file, "r").use { raf ->
            if (offset >= raf.length()) return ByteArray(0)
            raf.seek(offset)
            val buf = ByteArray(minOf(len.toLong(), raf.length() - offset).toInt())
            raf.readFully(buf)
            return buf
        }
    }

    override fun write(path: String, offset: Long, data: ByteArray) {
        val file = f(path)
        if (file.isDirectory) throw FsException.IsDir
        if (!file.exists()) throw FsException.NotFound
        RandomAccessFile(file, "rw").use { it.seek(offset); it.write(data) }
    }

    override fun create(path: String) {
        val file = f(path)
        if (!file.parentFile!!.isDirectory) throw FsException.NotFound
        if (!file.createNewFile()) throw FsException.Exists
    }

    override fun mkdir(path: String) {
        val file = f(path)
        if (file.exists()) throw FsException.Exists
        if (!file.mkdir()) throw FsException.Io
    }

    override fun remove(path: String) {
        val file = f(path)
        if (!file.exists()) throw FsException.NotFound
        if (file.isDirectory) throw FsException.IsDir
        if (!file.delete()) throw FsException.Io
    }

    override fun rmdir(path: String) {
        val file = f(path)
        if (file == root) throw FsException.Denied
        if (!file.exists()) throw FsException.NotFound
        if (!file.isDirectory) throw FsException.NotDir
        if (!file.list().isNullOrEmpty()) throw FsException.NotEmpty
        if (!file.delete()) throw FsException.Io
    }

    override fun rename(from: String, to: String) {
        val a = f(from)
        val b = f(to)
        if (a == root || b == root) throw FsException.Denied
        if (!a.exists()) throw FsException.NotFound
        if (b.isDirectory && !b.list().isNullOrEmpty()) throw FsException.NotEmpty
        if (b.exists() && b.isFile) b.delete() // POSIX rename replaces files
        if (!a.renameTo(b)) throw FsException.Io
    }

    override fun truncate(path: String, size: Long) {
        RandomAccessFile(f(path).also { if (!it.exists()) throw FsException.NotFound }, "rw").use { it.setLength(size) }
    }

    override fun statFs() = statFsOf(root)
}

// ── Shared folder: Storage Access Framework tree ─────────────────────────────

/**
 * Access to one user-picked folder through the Storage Access Framework, so
 * nothing outside it is reachable. Paths are resolved by walking child names
 * from the tree root; results are cached and invalidated on changes.
 */
class SafFs(private val ctx: Context, private val tree: Uri) : PhoneFs {
    private val cr: ContentResolver = ctx.contentResolver
    private val rootId: String = DocumentsContract.getTreeDocumentId(tree)
    private val idCache = HashMap<String, String>()

    private fun docUri(id: String): Uri = DocumentsContract.buildDocumentUriUsingTree(tree, id)

    private class Doc(val id: String, val name: String, val mime: String, val size: Long, val mtime: Long) {
        val dir get() = mime == Document.MIME_TYPE_DIR
        fun stat() = FsStat(dir, if (dir) 0 else size, mtime)
    }

    private val cols = arrayOf(
        Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
        Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED,
    )

    private fun query(uri: Uri): List<Doc> = cr.query(uri, cols, null, null, null)?.use { c ->
        buildList {
            while (c.moveToNext()) {
                add(Doc(c.getString(0), c.getString(1) ?: "", c.getString(2) ?: "", if (c.isNull(3)) 0 else c.getLong(3), if (c.isNull(4)) 0 else c.getLong(4)))
            }
        }
    } ?: throw FsException.Io

    private fun children(id: String): List<Doc> =
        query(DocumentsContract.buildChildDocumentsUriUsingTree(tree, id))

    private fun doc(id: String): Doc = query(docUri(id)).firstOrNull() ?: throw FsException.NotFound

    /**
     * Folder listings cached briefly by document id. File managers look up
     * every entry of a folder (and probe names like `.Trash`); without this,
     * each lookup re-lists the whole folder (~300 ms for a full Camera folder).
     */
    private class Listing(val at: Long, val byName: Map<String, Doc>)
    private val listings = HashMap<String, Listing>()
    private val listingTtlMs = 3_000L

    private fun childrenOf(id: String, fresh: Boolean = false): Map<String, Doc> {
        val now = android.os.SystemClock.elapsedRealtime()
        if (!fresh) synchronized(this) { listings[id]?.takeIf { now - it.at < listingTtlMs }?.let { return it.byName } }
        val byName = children(id).associateBy { it.name }
        synchronized(this) { listings[id] = Listing(now, byName) }
        return byName
    }

    /** Document id for [path], or NotFound. */
    @Synchronized
    private fun cachedId(path: String) = idCache[path]

    private fun idOf(path: String): String {
        val p = parts(path)
        if (p.isEmpty()) return rootId
        cachedId(path)?.let { return it }
        var id = rootId
        var sofar = ""
        for (name in p) {
            sofar = if (sofar.isEmpty()) name else "$sofar/$name"
            id = cachedId(sofar) ?: (childrenOf(id)[name]?.id ?: throw FsException.NotFound).also { synchronized(this) { idCache[sofar] = it } }
        }
        return id
    }

    /** Drops cached ids under [path] and the listing of its parent folder. */
    @Synchronized
    private fun forget(path: String) {
        idCache.keys.removeAll { it == path || it.startsWith("$path/") }
        val parent = path.substringBeforeLast('/', "")
        (if (parent.isEmpty()) rootId else idCache[parent])?.let { listings.remove(it) } ?: listings.clear()
    }

    private fun parentAndName(path: String): Pair<String, String> {
        val p = parts(path)
        if (p.isEmpty()) throw FsException.Denied
        return p.dropLast(1).joinToString("/") to p.last()
    }

    override fun stat(path: String): FsStat {
        if (parts(path).isEmpty()) return doc(rootId).stat()
        val (parent, name) = parentAndName(path)
        return (childrenOf(idOf(parent))[name] ?: throw FsException.NotFound).stat()
    }

    override fun list(path: String): List<Pair<String, FsStat>> {
        val id = idOf(path)
        if (id != rootId && !doc(id).dir) throw FsException.NotDir
        val byName = childrenOf(id, fresh = true)
        synchronized(this) { byName.forEach { (name, d) -> idCache[if (path.isEmpty()) name else "$path/$name"] = d.id } }
        return byName.values.filter { it.name.isNotEmpty() && '/' !in it.name }.map { it.name to it.stat() }
    }

    override fun read(path: String, offset: Long, len: Int): ByteArray {
        val id = idOf(path)
        val pfd = cr.openFileDescriptor(docUri(id), "r") ?: throw FsException.Io
        pfd.use {
            FileInputStream(it.fileDescriptor).channel.use { ch ->
                val size = ch.size()
                if (offset >= size) return ByteArray(0)
                val buf = java.nio.ByteBuffer.allocate(minOf(len.toLong(), size - offset).toInt())
                var pos = offset
                while (buf.hasRemaining()) {
                    val n = ch.read(buf, pos)
                    if (n <= 0) break
                    pos += n
                }
                return buf.array().copyOf(buf.position())
            }
        }
    }

    override fun write(path: String, offset: Long, data: ByteArray) {
        val pfd = cr.openFileDescriptor(docUri(idOf(path)), "rw") ?: throw FsException.Io
        pfd.use { FileOutputStream(it.fileDescriptor).channel.use { ch -> ch.write(java.nio.ByteBuffer.wrap(data), offset) } }
        dropListingOf(path) // size/mtime changed
    }

    @Synchronized
    private fun dropListingOf(path: String) {
        val parent = path.substringBeforeLast('/', "")
        (if (parent.isEmpty()) rootId else idCache[parent])?.let { listings.remove(it) }
    }

    private fun createChild(path: String, mime: String) {
        val (parent, name) = parentAndName(path)
        val pid = idOf(parent)
        if (name in childrenOf(pid, fresh = true)) throw FsException.Exists
        val uri = DocumentsContract.createDocument(cr, docUri(pid), mime, name) ?: throw FsException.Io
        synchronized(this) { listings.remove(pid) }
        // Providers may adjust the name (e.g. add an extension); map the path to what was created.
        synchronized(this) { idCache[path] = DocumentsContract.getDocumentId(uri) }
        val created = doc(DocumentsContract.getDocumentId(uri)).name
        if (created != name) {
            DocumentsContract.deleteDocument(cr, uri)
            forget(path)
            throw FsException.Denied // the provider won't keep this exact name
        }
    }

    override fun create(path: String) = createChild(path, "application/octet-stream")
    override fun mkdir(path: String) = createChild(path, Document.MIME_TYPE_DIR)

    override fun remove(path: String) {
        val id = idOf(path)
        if (doc(id).dir) throw FsException.IsDir
        if (!DocumentsContract.deleteDocument(cr, docUri(id))) throw FsException.Io
        forget(path)
    }

    override fun rmdir(path: String) {
        val id = idOf(path)
        if (id == rootId) throw FsException.Denied
        if (!doc(id).dir) throw FsException.NotDir
        if (children(id).isNotEmpty()) throw FsException.NotEmpty
        if (!DocumentsContract.deleteDocument(cr, docUri(id))) throw FsException.Io
        forget(path)
    }

    override fun rename(from: String, to: String) {
        val (fromParent, _) = parentAndName(from)
        val (toParent, toName) = parentAndName(to)
        val id = idOf(from)
        // POSIX rename replaces an existing file at the destination.
        runCatching { idOf(to) }.getOrNull()?.let { existing ->
            if (doc(existing).dir) throw FsException.Exists
            DocumentsContract.deleteDocument(cr, docUri(existing))
            forget(to)
        }
        var uri = docUri(id)
        if (fromParent != toParent) {
            uri = DocumentsContract.moveDocument(cr, uri, docUri(idOf(fromParent)), docUri(idOf(toParent))) ?: throw FsException.Io
        }
        if (doc(DocumentsContract.getDocumentId(uri)).name != toName) {
            uri = DocumentsContract.renameDocument(cr, uri, toName) ?: throw FsException.Io
        }
        forget(from)
        forget(to)
    }

    override fun truncate(path: String, size: Long) {
        val pfd = cr.openFileDescriptor(docUri(idOf(path)), "rw") ?: throw FsException.Io
        pfd.use { FileOutputStream(it.fileDescriptor).channel.use { ch -> ch.truncate(size) } }
        dropListingOf(path)
    }

    override fun statFs() = statFsOf(Environment.getExternalStorageDirectory())
}
