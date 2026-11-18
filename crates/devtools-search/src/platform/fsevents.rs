//! macOS 按卷回放 FSEvents 历史。事件只用于定位需要重新读取的路径。

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CStr, CString, OsStr};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devtools_search_core::snapshot::EventCheckpoint;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2_core_foundation::{CFArray, CFString, CFUUID};
use objc2_core_services as fs;

const MAX_PATHS: usize = 4096;
const REPLAY_TIMEOUT: Duration = Duration::from_secs(5);
const INVALID_HISTORY: u32 = fs::kFSEventStreamEventFlagMustScanSubDirs
    | fs::kFSEventStreamEventFlagUserDropped
    | fs::kFSEventStreamEventFlagKernelDropped
    | fs::kFSEventStreamEventFlagEventIdsWrapped
    | fs::kFSEventStreamEventFlagRootChanged
    | fs::kFSEventStreamEventFlagMount
    | fs::kFSEventStreamEventFlagUnmount;
const RELOCATED: u32 =
    fs::kFSEventStreamEventFlagItemRemoved | fs::kFSEventStreamEventFlagItemRenamed;
const STRUCTURAL: u32 = fs::kFSEventStreamEventFlagItemCreated | RELOCATED;
const ITEM_TYPE: u32 = fs::kFSEventStreamEventFlagItemIsDir
    | fs::kFSEventStreamEventFlagItemIsFile
    | fs::kFSEventStreamEventFlagItemIsSymlink;

/// 卷日志和根目录的身份，以及读取元数据前取得的保守检查点。
pub(crate) struct Journal {
    device: libc::dev_t,
    volume_path: PathBuf,
    identity: String,
    timestamp: u64,
    pub checkpoint: EventCheckpoint,
}

impl Journal {
    pub fn capture(root: &Path) -> io::Result<Self> {
        let (device, inode, identity, volume_path) = volume_root(root)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        let last_event_id = event_before(device, timestamp);
        if last_event_id == 0 || last_event_id == fs::kFSEventStreamEventIdSinceNow {
            return Err(io::Error::other("无法取得历史事件检查点"));
        }
        let identity = format!("fsevents-v1:{identity}:{inode}");
        let checkpoint = EventCheckpoint {
            last_event_id,
            stream_id: format!("{identity}:{timestamp}"),
        };
        Ok(Self {
            device,
            volume_path,
            identity,
            timestamp,
            checkpoint,
        })
    }

    /// 日志或根是否仍与检查点关联。APFS 数据卷的挂载别名可能变化，设备号不写进快照。
    pub fn still_current(&self, root: &Path) -> bool {
        volume_root(root).is_ok_and(|(dev, inode, uuid, volume_path)| {
            dev == self.device
                && volume_path == self.volume_path
                && format!("fsevents-v1:{uuid}:{inode}") == self.identity
                && event_before(dev, self.timestamp) >= self.checkpoint.last_event_id
        })
    }

    fn accepts(&self, previous: &EventCheckpoint) -> bool {
        let Some((identity, timestamp)) = previous.stream_id.rsplit_once(':') else {
            return false;
        };
        let Ok(timestamp) = timestamp.parse::<u64>() else {
            return false;
        };
        identity == self.identity
            && timestamp <= self.timestamp
            && previous.last_event_id > 0
            && previous.last_event_id <= self.checkpoint.last_event_id
            // 如果原来时点的历史不再覆盖原始检查点，不能以无事件判定索引已更新。
            && event_before(self.device, timestamp) >= previous.last_event_id
    }

    /// 返回绝对路径及其是否需要重新遍历子树。直到收到 HistoryDone 并排空当前批次才算成功。
    pub fn replay(
        &self,
        root: &Path,
        previous: &EventCheckpoint,
        cancel: &AtomicBool,
    ) -> io::Result<BTreeMap<PathBuf, bool>> {
        if !self.accepts(previous) {
            return Err(io::Error::other("FSEvents 检查点已失效或历史已不可读"));
        }
        let (signal, ready) = mpsc::sync_channel(1);
        let stream = Stream::start(self, root, previous.last_event_id, signal)?;
        let deadline = Instant::now() + REPLAY_TIMEOUT;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "回放已取消"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "等待 FSEvents 历史回放超时",
                ));
            }
            match ready.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(err) => return Err(io::Error::other(err)),
            }
        }
        if stream.state.lock().unwrap().invalid {
            return Err(io::Error::other("FSEvents 要求重新扫描"));
        }
        // 安全性：流已经启动，当前线程不是回调队列。同步排空已经等待交付的事件，
        // 再在串行队列上确认回调结束，覆盖 HistoryDone 附近的变化。
        unsafe { fs::FSEventStreamFlushSync(stream.raw) };
        stream.queue.exec_sync(|| {});
        let paths = {
            let mut state = stream.state.lock().unwrap();
            if state.invalid || !state.complete {
                return Err(io::Error::other("FSEvents 回放不完整"));
            }
            std::mem::take(&mut state.paths)
        };
        drop(stream);
        if !self.still_current(root) {
            return Err(io::Error::other("FSEvents 回放期间卷已变化"));
        }
        paths
            .into_iter()
            .map(|(path, flags)| {
                if flags & fs::kFSEventStreamEventFlagItemRenamed != 0 {
                    verify_rename_spelling(&path)?;
                }
                Ok((path, needs_subtree(flags)))
            })
            .collect()
    }
}

fn event_before(device: libc::dev_t, timestamp: u64) -> u64 {
    // 安全性：设备号来自 stat。SDK 对此 API 指定 Unix 时间，不用 CFDate 的 2001 年纪元。
    unsafe { fs::FSEventsGetLastEventIdForDeviceBeforeTime(device, timestamp as f64) }
}

/// 大小写不敏感的卷上，仅改大小写后旧拼写仍能 stat。
/// 如果实际名字不同，回退到扫描，避免把旧、新拼写都保留在索引里。
fn verify_rename_spelling(path: &Path) -> io::Result<()> {
    let encoded = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let mut attrs = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_NAME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut output = [0u8; 1040];
    // 安全性：输入路径以 NUL 结尾，输出缓冲区大小明确。只读取名称，不跟随末端符号链接。
    let result = unsafe {
        libc::getattrlist(
            encoded.as_ptr(),
            std::ptr::from_mut(&mut attrs).cast(),
            output.as_mut_ptr().cast(),
            output.len(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    if result != 0 {
        let err = io::Error::last_os_error();
        return if err.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(err)
        };
    }
    // 返回布局为 u32 长度、attrreference_t，再跟名称。偏移相对 attrreference_t 开头。
    let offset = i32::from_ne_bytes(output[4..8].try_into().unwrap());
    let size = u32::from_ne_bytes(output[8..12].try_into().unwrap()) as usize;
    let name = usize::try_from(i64::from(offset) + 4)
        .ok()
        .and_then(|offset| output.get(offset..offset.checked_add(size)?))
        .and_then(|name| CStr::from_bytes_with_nul(name).ok())
        .map(|name| OsStr::from_bytes(name.to_bytes()));
    if name.is_none() || name != path.file_name() {
        return Err(io::Error::other("重命名后的路径拼写需要重新确认"));
    }
    Ok(())
}

fn needs_subtree(flags: u32) -> bool {
    flags & ITEM_TYPE == 0
        || flags & fs::kFSEventStreamEventFlagItemIsDir != 0 && flags & STRUCTURAL != 0
}

fn volume_root(root: &Path) -> io::Result<(libc::dev_t, u64, String, PathBuf)> {
    let meta = std::fs::metadata(root)?;
    let device = meta.dev() as libc::dev_t;
    if !meta.is_dir() || device == 0 {
        return Err(io::Error::other("索引根不在可回放的卷上"));
    }
    let path = CString::new(root.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let mut stats = MaybeUninit::<libc::statfs>::uninit();
    // 安全性：path 以 NUL 结尾，statfs 成功时初始化输出。
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    let mount = unsafe { CStr::from_ptr(stats.f_mntonname.as_ptr()) };
    let mount = Path::new(OsStr::from_bytes(mount.to_bytes()));
    let volume_path = root
        .strip_prefix(mount)
        .or_else(|err| {
            // APFS Data 卷也经由 firmlink 出现在 /Users、/private 等系统路径下。
            if mount == Path::new("/System/Volumes/Data") {
                root.strip_prefix("/")
            } else {
                Err(err)
            }
        })
        .map_err(io::Error::other)?;
    // 用实际挂载路径验证 firmlink 推导，避免监听另一个同名目录。
    let underlying = std::fs::metadata(mount.join(volume_path))?;
    if (underlying.dev(), underlying.ino()) != (meta.dev(), meta.ino()) {
        return Err(io::Error::other("索引根的卷相对路径无法确认"));
    }
    // 安全性：设备号来自有效目录的 stat。
    let uuid = unsafe { fs::FSEventsCopyUUIDForDevice(device) }
        .and_then(|uuid| CFUUID::new_string(None, Some(&uuid)))
        .ok_or_else(|| io::Error::other("卷没有持久 FSEvents 日志"))?;
    Ok((
        device,
        meta.ino(),
        uuid.to_string(),
        volume_path.to_path_buf(),
    ))
}

struct ReplayState {
    root: PathBuf,
    relative: PathBuf,
    paths: BTreeMap<PathBuf, u32>,
    signal: mpsc::SyncSender<()>,
    complete: bool,
    invalid: bool,
}

impl ReplayState {
    fn invalidate(&mut self) {
        self.invalid = true;
        let _ = self.signal.try_send(());
    }

    fn record(&mut self, path: &Path, flags: u32) {
        if self.invalid {
            return;
        }
        if flags & INVALID_HISTORY != 0 {
            self.invalidate();
            return;
        }
        if flags & fs::kFSEventStreamEventFlagHistoryDone != 0 {
            self.complete = true;
            let _ = self.signal.try_send(());
            return;
        }
        let path = path.strip_prefix("/").unwrap_or(path);
        if path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            self.invalidate();
            return;
        }
        let Ok(relative) = path.strip_prefix(&self.relative) else {
            // 上级目录的结构变化可能影响整个索引根。
            if self.relative.starts_with(path) && flags & RELOCATED != 0 {
                self.invalidate();
            }
            return;
        };
        if relative.as_os_str().is_empty() {
            // 重叠历史中可能有根目录当初的创建事件。根 inode 已验证，
            // 单纯创建不会使快照失效。移动、删除和没有类型的粗粒度事件仍须兜底。
            if flags & ITEM_TYPE == 0 || flags & RELOCATED != 0 {
                self.invalidate();
            }
            return;
        }
        *self.paths.entry(self.root.join(relative)).or_default() |= flags;
        if self.paths.len() > MAX_PATHS {
            self.invalidate();
        }
    }
}

struct Stream {
    raw: fs::FSEventStreamRef,
    queue: DispatchRetained<DispatchQueue>,
    state: Box<Mutex<ReplayState>>,
    started: bool,
}

impl Stream {
    fn start(
        journal: &Journal,
        root: &Path,
        since: u64,
        signal: mpsc::SyncSender<()>,
    ) -> io::Result<Self> {
        let relative = journal
            .volume_path
            .to_str()
            .ok_or_else(|| io::Error::other("FSEvents 索引根无法编码"))?;
        let cf_path = CFString::from_str(relative);
        let paths = CFArray::from_objects(&[&*cf_path]);
        let queue = DispatchQueue::new("devtools-search.replay", DispatchQueueAttr::SERIAL);
        let mut state = Box::new(Mutex::new(ReplayState {
            root: root.to_path_buf(),
            relative: journal.volume_path.clone(),
            paths: BTreeMap::new(),
            signal,
            complete: false,
            invalid: false,
        }));
        let mut context = fs::FSEventStreamContext {
            version: 0,
            info: std::ptr::from_mut(&mut *state).cast(),
            retain: None,
            release: None,
            copyDescription: None,
        };
        // 安全性：paths 包含一个卷相对 CFString，context 指向的堆分配由 Stream 保活。
        let raw = unsafe {
            fs::FSEventStreamCreateRelativeToDevice(
                None,
                Some(event_callback),
                &mut context,
                journal.device,
                paths.as_opaque(),
                since,
                0.05,
                fs::kFSEventStreamCreateFlagFileEvents
                    | fs::kFSEventStreamCreateFlagNoDefer
                    | fs::kFSEventStreamCreateFlagWatchRoot
                    | fs::kFSEventStreamCreateFlagFullHistory,
            )
        };
        if raw.is_null() {
            return Err(io::Error::other("无法创建 FSEvents 历史流"));
        }
        let mut stream = Self {
            raw,
            queue,
            state,
            started: false,
        };
        // 安全性：流有效且队列在整个流的生命周期内存在。
        unsafe {
            fs::FSEventStreamSetDispatchQueue(raw, Some(&stream.queue));
            stream.started = fs::FSEventStreamStart(raw);
        }
        if stream.started {
            Ok(stream)
        } else {
            Err(io::Error::other("无法启动 FSEvents 历史流"))
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // 安全性：只有创建流的工作线程停止它。等串行队列排空后才释放回调数据。
        unsafe {
            if self.started {
                fs::FSEventStreamStop(self.raw);
            }
            fs::FSEventStreamInvalidate(self.raw);
        }
        self.queue.exec_sync(|| {});
        unsafe { fs::FSEventStreamRelease(self.raw) };
    }
}

unsafe extern "C-unwind" fn event_callback(
    _: fs::ConstFSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: NonNull<c_void>,
    flags: NonNull<u32>,
    _: NonNull<u64>,
) {
    // 安全性：未设置 UseCFTypes，SDK 保证 paths 是 count 个 C 字符串组成的数组。
    // info 由 Stream 的 state 提供，停止流并排空回调队列之前始终有效。
    let state = unsafe { &*info.cast::<Mutex<ReplayState>>() };
    let Ok(mut state) = state.lock() else {
        return;
    };
    let paths =
        unsafe { std::slice::from_raw_parts(paths.cast::<*const c_char>().as_ptr(), count) };
    let flags = unsafe { std::slice::from_raw_parts(flags.as_ptr(), count) };
    for (&path, &flags) in paths.iter().zip(flags) {
        if path.is_null() && flags & fs::kFSEventStreamEventFlagHistoryDone == 0 {
            state.invalidate();
            break;
        }
        let path = if path.is_null() {
            Path::new("")
        } else {
            Path::new(OsStr::from_bytes(
                unsafe { CStr::from_ptr(path) }.to_bytes(),
            ))
        };
        state.record(path, flags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_batches_deduplicate_paths_and_reject_missing_history() {
        let (signal, ready) = mpsc::sync_channel(1);
        let mut state = ReplayState {
            root: "/mnt/base".into(),
            relative: "base".into(),
            paths: BTreeMap::new(),
            signal,
            complete: false,
            invalid: false,
        };
        state.record(Path::new("/base/dir"), fs::kFSEventStreamEventFlagItemIsDir);
        state.record(
            Path::new("base/dir"),
            fs::kFSEventStreamEventFlagItemIsDir | fs::kFSEventStreamEventFlagItemRenamed,
        );
        state.record(
            Path::new("elsewhere/file"),
            fs::kFSEventStreamEventFlagItemIsFile,
        );
        state.record(Path::new(""), fs::kFSEventStreamEventFlagHistoryDone);
        assert!(ready.try_recv().is_ok());
        assert!(state.complete && !state.invalid);
        assert_eq!(
            state.paths,
            BTreeMap::from([(
                PathBuf::from("/mnt/base/dir"),
                fs::kFSEventStreamEventFlagItemIsDir | fs::kFSEventStreamEventFlagItemRenamed,
            )])
        );
        state.record(
            Path::new("base"),
            fs::kFSEventStreamEventFlagMustScanSubDirs,
        );
        assert!(state.invalid && ready.try_recv().is_ok());
    }
}
