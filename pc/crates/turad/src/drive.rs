//! The phone as a drive: a FUSE file system at
//! `$XDG_RUNTIME_DIR/tunnelrat/<phone name>` whose every call is forwarded to
//! the phone over its (paired, mutual-TLS) connection. A bookmark puts it in
//! GNOME Files' sidebar.
//!
//! Not in $HOME on purpose: anything that walks the home folder (indexers,
//! the dock's drive list) would touch it, and a FUSE call that waits for an
//! unreachable phone can't be frozen, which hangs suspend. For the same
//! reason calls fail fast once the phone stops answering, and the mount is
//! taken down before the PC sleeps (see `sleep.rs`).
//!
//! FUSE mounts are private to the mounting user by default, so other local
//! users can't browse the phone.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, OpenFlags, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tura_proto::{DirEntry, FileStat, FsError, FsOp, FsResult, Message};

const TTL: Duration = Duration::from_secs(1);
/// How long stats from a directory listing answer lookups without asking the phone.
const ATTR_CACHE: Duration = Duration::from_secs(3);
/// A call the phone hasn't answered by then fails (EIO-like ETIMEDOUT)...
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

// ── Request routing ──────────────────────────────────────

/// Sends file requests into one phone session and matches the replies.
/// A reply: JSON for everything except read data, which comes as raw bytes.
enum Reply {
    Json(FsResult),
    Bytes(Vec<u8>),
}

pub struct DriveLink {
    tx: mpsc::UnboundedSender<WsMessage>,
    /// Plain thread channels: FUSE workers wait without touching tokio, so a
    /// shutting-down runtime can't make them panic.
    pending: Mutex<HashMap<u64, std::sync::mpsc::SyncSender<Reply>>>,
    next: AtomicU64,
    /// ...and after such a timeout, further calls fail at once (ENOTCONN)
    /// until the phone answers again, so programs don't queue up behind it.
    unresponsive: std::sync::atomic::AtomicBool,
    last_probe: Mutex<std::time::Instant>,
}

impl DriveLink {
    pub fn new(tx: mpsc::UnboundedSender<WsMessage>) -> Arc<Self> {
        Arc::new(Self { tx, pending: Mutex::new(HashMap::new()), next: AtomicU64::new(1), unresponsive: Default::default(), last_probe: Mutex::new(std::time::Instant::now()) })
    }

    /// Blocking call for FUSE worker threads (never call from async code).
    fn call(&self, op: FsOp) -> Result<FsResult, Errno> {
        match self.request(|id| WsMessage::Text(Message::FsRequest { id, op }.to_json().into()))? {
            Reply::Json(r) => Ok(r),
            Reply::Bytes(_) => Err(Errno::EIO),
        }
    }

    fn read(&self, path: String, offset: u64, len: u32) -> Result<Vec<u8>, Errno> {
        match self.request(|id| WsMessage::Text(Message::FsRequest { id, op: FsOp::Read { path, offset, len } }.to_json().into()))? {
            Reply::Bytes(b) => Ok(b),
            Reply::Json(_) => Err(Errno::EIO),
        }
    }

    fn write(&self, path: &str, offset: u64, data: &[u8]) -> Result<(), Errno> {
        self.request(|id| WsMessage::Binary(tura_proto::bin::write(id, path, offset, data).into())).map(|_| ())
    }

    fn request(&self, make: impl FnOnce(u64) -> WsMessage) -> Result<Reply, Errno> {
        if self.unresponsive.load(Ordering::Relaxed) {
            // Probe now and then so the drive comes back once the phone does.
            let mut last = self.last_probe.lock().unwrap();
            if last.elapsed() >= Duration::from_secs(2) {
                *last = std::time::Instant::now();
                let id = self.next.fetch_add(1, Ordering::Relaxed);
                let _ = self.tx.send(WsMessage::Text(Message::FsRequest { id, op: FsOp::StatFs }.to_json().into()));
            }
            return Err(Errno::ENOTCONN);
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.pending.lock().unwrap().insert(id, tx);
        if self.tx.send(make(id)).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(Errno::ENOTCONN);
        }
        match rx.recv_timeout(CALL_TIMEOUT) {
            Ok(Reply::Json(FsResult::Error { error })) => Err(errno(error)),
            Ok(r) => Ok(r),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(Errno::ENOTCONN), // session ended
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                tracing::warn!("phone drive: no answer for {} s; failing calls until it answers", CALL_TIMEOUT.as_secs());
                self.unresponsive.store(true, Ordering::Relaxed);
                Err(Errno::ETIMEDOUT)
            }
        }
    }

    pub fn complete(&self, id: u64, result: FsResult) {
        self.unresponsive.store(false, Ordering::Relaxed);
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.try_send(Reply::Json(result));
        }
    }

    /// A binary frame from the phone (read data).
    pub fn complete_binary(&self, frame: &[u8]) {
        self.unresponsive.store(false, Ordering::Relaxed);
        if let Some((tura_proto::bin::READ_DATA, id, data)) = tura_proto::bin::split(frame)
            && let Some(tx) = self.pending.lock().unwrap().remove(&id)
        {
            let _ = tx.try_send(Reply::Bytes(data.to_vec()));
        }
    }

    /// Fails every waiting call (the phone disconnected).
    pub fn fail_all(&self) {
        self.pending.lock().unwrap().clear();
    }
}

fn errno(e: FsError) -> Errno {
    match e {
        FsError::NotFound => Errno::ENOENT,
        FsError::Exists => Errno::EEXIST,
        FsError::NotEmpty => Errno::ENOTEMPTY,
        FsError::NotDir => Errno::ENOTDIR,
        FsError::IsDir => Errno::EISDIR,
        FsError::Denied => Errno::EACCES,
        FsError::NoSpace => Errno::ENOSPC,
        FsError::Io => Errno::EIO,
    }
}

// ── FUSE file system ─────────────────────────────────────

#[derive(Default)]
struct Inodes {
    by_ino: HashMap<u64, String>,
    by_path: HashMap<String, u64>,
    next: u64,
}

impl Inodes {
    fn new() -> Self {
        let mut i = Self { next: 2, ..Default::default() };
        i.by_ino.insert(1, String::new());
        i.by_path.insert(String::new(), 1);
        i
    }

    fn ino(&mut self, path: &str) -> u64 {
        if let Some(&i) = self.by_path.get(path) {
            return i;
        }
        let i = self.next;
        self.next += 1;
        self.by_ino.insert(i, path.to_owned());
        self.by_path.insert(path.to_owned(), i);
        i
    }

    fn forget_path(&mut self, path: &str) {
        if let Some(i) = self.by_path.remove(path) {
            self.by_ino.remove(&i);
        }
    }

    /// Moves `from` (and everything below it) to `to`, keeping inode numbers.
    fn rename(&mut self, from: &str, to: &str) {
        self.forget_path(to);
        let prefix = format!("{from}/");
        let moved: Vec<(String, u64)> =
            self.by_path.iter().filter(|(p, _)| *p == from || p.starts_with(&prefix)).map(|(p, &i)| (p.clone(), i)).collect();
        for (old, i) in moved {
            let new = format!("{to}{}", &old[from.len()..]);
            self.by_path.remove(&old);
            self.by_path.insert(new.clone(), i);
            self.by_ino.insert(i, new);
        }
    }
}

/// One cached directory listing: (inode, kind, name) per entry.
type Listing = Vec<(u64, FileType, String)>;

pub struct PhoneFs {
    link: Arc<DriveLink>,
    inodes: Mutex<Inodes>,
    /// Directory listings held from opendir to releasedir (fh → entries), so
    /// large folders aren't re-listed for every readdir page.
    dirs: Mutex<HashMap<u64, Listing>>,
    next_fh: AtomicU64,
    /// Stats learned from directory listings, so `ls -l` / file managers
    /// don't need one round trip per entry. Short-lived; dropped on changes.
    attrs: Mutex<HashMap<String, (FileStat, std::time::Instant)>>,
    uid: u32,
    gid: u32,
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") }
}

/// Plain names only: no separators, no `.`/`..`, valid UTF-8.
fn name_str(name: &OsStr) -> Result<&str, Errno> {
    let n = name.to_str().ok_or(Errno::from_i32(libc::EINVAL))?;
    if n.is_empty() || n == "." || n == ".." || n.contains('/') {
        return Err(Errno::from_i32(libc::EINVAL));
    }
    Ok(n)
}

impl PhoneFs {
    pub fn new(link: Arc<DriveLink>) -> Self {
        // SAFETY: getuid/getgid have no preconditions and can't fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Self {
            link,
            inodes: Mutex::new(Inodes::new()),
            dirs: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            attrs: Mutex::new(HashMap::new()),
            uid,
            gid,
        }
    }

    fn path(&self, ino: INodeNo) -> Result<String, Errno> {
        self.inodes.lock().unwrap().by_ino.get(&ino.0).cloned().ok_or(Errno::ENOENT)
    }

    fn attr(&self, ino: u64, s: &FileStat) -> FileAttr {
        let t = UNIX_EPOCH + Duration::from_millis(s.mtime_ms.max(0) as u64);
        FileAttr {
            ino: INodeNo(ino),
            size: s.size,
            blocks: s.size.div_ceil(512),
            atime: t,
            mtime: t,
            ctime: t,
            crtime: t,
            kind: if s.dir { FileType::Directory } else { FileType::RegularFile },
            perm: if s.dir { 0o755 } else { 0o644 },
            nlink: if s.dir { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn stat(&self, path: &str) -> Result<FileStat, Errno> {
        if let Some((s, at)) = self.attrs.lock().unwrap().get(path)
            && at.elapsed() < ATTR_CACHE
        {
            return Ok(s.clone());
        }
        match self.link.call(FsOp::Stat { path: path.to_owned() })? {
            FsResult::Stat { stat } => Ok(stat),
            _ => Err(Errno::EIO),
        }
    }

    /// Forget cached stats for `path` (and anything below it).
    fn invalidate(&self, path: &str) {
        let prefix = format!("{path}/");
        self.attrs.lock().unwrap().retain(|p, _| p != path && !p.starts_with(&prefix));
    }

    fn entry_for(&self, path: &str) -> Result<FileAttr, Errno> {
        let stat = self.stat(path)?;
        let ino = self.inodes.lock().unwrap().ino(path);
        Ok(self.attr(ino, &stat))
    }

    fn done(&self, op: FsOp) -> Result<(), Errno> {
        self.link.call(op).map(|_| ())
    }
}

impl Filesystem for PhoneFs {
    /// Ask the kernel for 1 MiB reads/writes instead of 128 KiB: every request
    /// is a round trip to the phone, so fewer, bigger ones are much faster.
    fn init(&mut self, _req: &Request, cfg: &mut fuser::KernelConfig) -> std::io::Result<()> {
        let _ = cfg.add_capabilities(fuser::InitFlags::FUSE_MAX_PAGES);
        let _ = cfg.set_max_write(tura_proto::FS_CHUNK);
        if let Err(max) = cfg.set_max_readahead(tura_proto::FS_CHUNK) {
            let _ = cfg.set_max_readahead(max);
        }
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let r = (|| {
            let p = join(&self.path(parent)?, name_str(name)?);
            self.entry_for(&p)
        })();
        match r {
            Ok(a) => reply.entry(&TTL, &a, Generation(0)),
            Err(e) => reply.error(e),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.path(ino).and_then(|p| self.stat(&p)) {
            Ok(s) => reply.attr(&TTL, &self.attr(ino.0, &s)),
            Err(e) => reply.error(e),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        // Only truncation is meaningful on the phone; mode/owner/times are accepted and ignored.
        let r = (|| {
            let p = self.path(ino)?;
            if let Some(size) = size {
                self.invalidate(&p);
                self.done(FsOp::Truncate { path: p.clone(), size })?;
            }
            self.stat(&p)
        })();
        match r {
            Ok(s) => reply.attr(&TTL, &self.attr(ino.0, &s)),
            Err(e) => reply.error(e),
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let r = (|| {
            let p = self.path(ino)?;
            let entries: Vec<DirEntry> = match self.link.call(FsOp::List { path: p.clone() })? {
                FsResult::List { entries } => entries,
                _ => return Err(Errno::EIO),
            };
            {
                let now = std::time::Instant::now();
                let mut attrs = self.attrs.lock().unwrap();
                if attrs.len() > 50_000 {
                    attrs.clear(); // bound memory on huge trees
                }
                for e in &entries {
                    if !e.name.is_empty() && !e.name.contains('/') {
                        attrs.insert(join(&p, &e.name), (e.stat.clone(), now));
                    }
                }
            }
            let mut inodes = self.inodes.lock().unwrap();
            let parent_ino = if p.is_empty() { 1 } else { inodes.ino(p.rsplit_once('/').map(|(a, _)| a).unwrap_or("")) };
            let mut list = vec![(ino.0, FileType::Directory, ".".to_owned()), (parent_ino, FileType::Directory, "..".to_owned())];
            for e in entries {
                if e.name.is_empty() || e.name.contains('/') || e.name == "." || e.name == ".." {
                    continue; // never trust names from the other side
                }
                let kind = if e.stat.dir { FileType::Directory } else { FileType::RegularFile };
                list.push((inodes.ino(&join(&p, &e.name)), kind, e.name));
            }
            drop(inodes);
            let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
            self.dirs.lock().unwrap().insert(fh, list);
            Ok(fh)
        })();
        match r {
            Ok(fh) => reply.opened(FileHandle(fh), FopenFlags::empty()),
            Err(e) => reply.error(e),
        }
    }

    fn readdir(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, offset: u64, mut reply: ReplyDirectory) {
        let dirs = self.dirs.lock().unwrap();
        let Some(list) = dirs.get(&fh.0) else { return reply.error(Errno::EBADF) };
        for (i, (ino, kind, name)) in list.iter().enumerate().skip(offset as usize) {
            if reply.add(INodeNo(*ino), (i + 1) as u64, *kind, name) {
                break; // buffer full; the kernel asks again with a later offset
            }
        }
        reply.ok();
    }

    fn releasedir(&self, _req: &Request, _ino: INodeNo, fh: FileHandle, _flags: OpenFlags, reply: ReplyEmpty) {
        self.dirs.lock().unwrap().remove(&fh.0);
        reply.ok();
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        let r = (|| {
            let path = self.path(ino)?;
            self.link.read(path, offset, size.min(tura_proto::FS_CHUNK))
        })();
        match r {
            Ok(d) => reply.data(&d),
            Err(e) => reply.error(e),
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _wflags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock: Option<fuser::LockOwner>,
        reply: ReplyWrite,
    ) {
        let r = (|| {
            let path = self.path(ino)?;
            self.invalidate(&path); // size/mtime change
            for (i, chunk) in data.chunks(tura_proto::FS_CHUNK as usize).enumerate() {
                let at = offset + (i * tura_proto::FS_CHUNK as usize) as u64;
                self.link.write(&path, at, chunk)?;
            }
            Ok(())
        })();
        match r {
            Ok(()) => reply.written(data.len() as u32),
            Err(e) => reply.error(e),
        }
    }

    fn create(&self, _req: &Request, parent: INodeNo, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        let r = (|| {
            let p = join(&self.path(parent)?, name_str(name)?);
            self.invalidate(&p);
            self.done(FsOp::Create { path: p.clone() })?;
            self.entry_for(&p)
        })();
        match r {
            Ok(a) => reply.created(&TTL, &a, Generation(0), FileHandle(0), FopenFlags::empty()),
            Err(e) => reply.error(e),
        }
    }

    fn mkdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        let r = (|| {
            let p = join(&self.path(parent)?, name_str(name)?);
            self.invalidate(&p);
            self.done(FsOp::Mkdir { path: p.clone() })?;
            self.entry_for(&p)
        })();
        match r {
            Ok(a) => reply.entry(&TTL, &a, Generation(0)),
            Err(e) => reply.error(e),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let r = (|| {
            let p = join(&self.path(parent)?, name_str(name)?);
            self.invalidate(&p);
            self.done(FsOp::Remove { path: p.clone() })?;
            self.inodes.lock().unwrap().forget_path(&p);
            Ok(())
        })();
        match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let r = (|| {
            let p = join(&self.path(parent)?, name_str(name)?);
            self.invalidate(&p);
            self.done(FsOp::Rmdir { path: p.clone() })?;
            self.inodes.lock().unwrap().forget_path(&p);
            Ok(())
        })();
        match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: fuser::RenameFlags,
        reply: ReplyEmpty,
    ) {
        let r = (|| {
            let from = join(&self.path(parent)?, name_str(name)?);
            let to = join(&self.path(newparent)?, name_str(newname)?);
            self.invalidate(&from);
            self.invalidate(&to);
            self.done(FsOp::Rename { from: from.clone(), to: to.clone() })?;
            self.inodes.lock().unwrap().rename(&from, &to);
            Ok(())
        })();
        match r {
            Ok(()) => reply.ok(),
            Err(e) => reply.error(e),
        }
    }

    fn flush(&self, _req: &Request, _ino: INodeNo, _fh: FileHandle, _lock: fuser::LockOwner, reply: ReplyEmpty) {
        reply.ok(); // writes are forwarded synchronously
    }

    fn fsync(&self, _req: &Request, _ino: INodeNo, _fh: FileHandle, _datasync: bool, reply: ReplyEmpty) {
        reply.ok();
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        match self.link.call(FsOp::StatFs) {
            Ok(FsResult::StatFs { total, free }) => reply.statfs(total / 4096, free / 4096, free / 4096, 0, 0, 4096, 255, 4096),
            Ok(_) => reply.error(Errno::EIO),
            Err(e) => reply.error(e),
        }
    }
}

// ── Mounting ─────────────────────────────────────────────

pub struct Mounted {
    session: fuser::BackgroundSession,
    pub dir: PathBuf,
    /// The label it was offered under (re-offers of the same label are no-ops).
    pub label: String,
}

/// `$XDG_RUNTIME_DIR/tunnelrat`: per-user, private, gone at logout.
fn mount_root() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("tunnelrat")
}

fn bookmarks_file() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/gtk-3.0/bookmarks")
}

fn file_uri(dir: &Path) -> String {
    let mut s = String::from("file://");
    for b in dir.as_os_str().as_encoded_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(*b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// Adds (or removes) a GNOME Files sidebar bookmark for a mount, leaving the
/// user's other bookmarks alone.
fn bookmark(dir: &Path, label: &str, add: bool) {
    let path = bookmarks_file();
    let uri = file_uri(dir);
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = old.lines().filter(|l| l.split(' ').next() != Some(uri.as_str())).collect();
    let entry = format!("{uri} {label}");
    if add {
        lines.push(&entry);
    }
    let new = lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    if new != old {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(&path, new);
    }
}

/// "Pixel 7" → a safe folder name.
fn folder_name(label: &str) -> String {
    let n: String = label.chars().filter(|c| !c.is_control() && *c != '/').collect();
    let n = n.trim().trim_start_matches('.').trim();
    if n.is_empty() { "Phone".into() } else { n.chars().take(60).collect() }
}

/// Lazily unmounts a leftover mount (e.g. after turad crashed: "Transport
/// endpoint is not connected").
fn clear_stale(dir: &Path) {
    let _ = std::process::Command::new("fusermount3").arg("-u").arg("-z").arg(dir).status();
}

/// Mounts the phone at `$XDG_RUNTIME_DIR/tunnelrat/<label>` and bookmarks it
/// (blocking; run on a blocking thread).
pub fn mount(label: &str, link: Arc<DriveLink>, registry: &Mutex<HashSet<PathBuf>>) -> Result<Mounted> {
    let base = folder_name(label);
    let root = mount_root();
    std::fs::create_dir_all(&root)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
    }
    let dir = root.join(&base);
    if std::fs::metadata(&dir).is_err() && std::fs::symlink_metadata(&dir).is_ok() {
        clear_stale(&dir); // exists but can't be stat'ed: a dead FUSE mount
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut cfg = fuser::Config::default();
    cfg.mount_options = vec![
        fuser::MountOption::FSName(format!("tunnelrat:{base}")),
        fuser::MountOption::Subtype("tunnelrat".into()),
        fuser::MountOption::NoDev,
        fuser::MountOption::NoSuid,
        fuser::MountOption::NoExec,
    ];
    cfg.n_threads = Some(4);
    let session = fuser::spawn_mount(PhoneFs::new(link), &dir, &cfg).with_context(|| format!("mounting {}", dir.display()))?;
    registry.lock().unwrap().insert(dir.clone());
    bookmark(&dir, &base, true);
    Ok(Mounted { session, dir, label: label.to_owned() })
}

/// Unmounts and removes the (now empty) mount folder (blocking).
pub fn unmount(m: Mounted, registry: &Mutex<HashSet<PathBuf>>) {
    let dir = m.dir.clone();
    if let Err(e) = m.session.umount_and_join() {
        tracing::warn!("unmounting {}: {e}", dir.display());
        clear_stale(&dir);
    }
    registry.lock().unwrap().remove(&dir);
    bookmark(&dir, "", false);
    let _ = std::fs::remove_dir(&dir); // only succeeds if empty, i.e. really unmounted
}

/// Shutdown path: unmount everything still mounted.
pub fn unmount_all(registry: &Mutex<HashSet<PathBuf>>) {
    for dir in registry.lock().unwrap().drain() {
        clear_stale(&dir);
        bookmark(&dir, "", false);
        let _ = std::fs::remove_dir(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inode_rename_moves_subtree() {
        let mut i = Inodes::new();
        let a = i.ino("DCIM");
        let b = i.ino("DCIM/Camera/x.jpg");
        let c = i.ino("DCIMX"); // shares a prefix but isn't inside
        i.rename("DCIM", "Photos");
        assert_eq!(i.by_ino[&a], "Photos");
        assert_eq!(i.by_ino[&b], "Photos/Camera/x.jpg");
        assert_eq!(i.by_ino[&c], "DCIMX");
        assert_eq!(i.by_path["Photos/Camera/x.jpg"], b);
        assert!(!i.by_path.contains_key("DCIM"));
    }

    #[test]
    fn names_are_validated() {
        assert!(name_str(OsStr::new("ok.txt")).is_ok());
        for bad in ["", ".", "..", "a/b"] {
            assert!(name_str(OsStr::new(bad)).is_err(), "{bad:?}");
        }
        assert_eq!(folder_name("Pixel 7"), "Pixel 7");
        assert_eq!(folder_name("../x/y"), "xy");
        assert_eq!(folder_name("  "), "Phone");
    }
}
