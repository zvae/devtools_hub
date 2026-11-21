//! Windows 启动校验。只读取本地 NTFS 已存在的 USN Journal，不创建日志或调整权限。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use devtools_search_core::snapshot::EventCheckpoint;
use windows_sys::Win32::Foundation::{GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FileIdType, GetFileInformationByHandle, GetFinalPathNameByHandleW,
    GetVolumeInformationByHandleW, OpenFileById, BY_HANDLE_FILE_INFORMATION,
    FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, VOLUME_NAME_GUID,
};
use windows_sys::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V1, USN_JOURNAL_DATA_V0,
    USN_JOURNAL_DATA_V1, USN_REASON_BASIC_INFO_CHANGE, USN_REASON_DATA_EXTEND,
    USN_REASON_DATA_OVERWRITE, USN_REASON_DATA_TRUNCATION, USN_REASON_FILE_CREATE,
    USN_REASON_FILE_DELETE, USN_REASON_HARD_LINK_CHANGE, USN_REASON_RENAME_NEW_NAME,
    USN_REASON_RENAME_OLD_NAME, USN_REASON_REPARSE_POINT_CHANGE, USN_REASON_SECURITY_CHANGE,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RENAME: u32 = USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME;
const STRUCTURE: u32 = USN_REASON_FILE_CREATE
    | USN_REASON_FILE_DELETE
    | RENAME
    | USN_REASON_REPARSE_POINT_CHANGE
    | USN_REASON_SECURITY_CHANGE;
const ALIAS_METADATA: u32 = USN_REASON_BASIC_INFO_CHANGE
    | USN_REASON_DATA_EXTEND
    | USN_REASON_DATA_OVERWRITE
    | USN_REASON_DATA_TRUNCATION;
const REASONS: u32 = STRUCTURE | ALIAS_METADATA | USN_REASON_HARD_LINK_CHANGE;
const MAX_JOURNAL_SPAN: i64 = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 65_536;
const MAX_PATHS: usize = 4096;
const REPLAY_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Journal {
    volume: OwnedHandle,
    root: OwnedHandle,
    root_path: PathBuf,
    root_id: u64,
    state: JournalState,
    pub checkpoint: EventCheckpoint,
}

impl Journal {
    pub fn capture(path: &Path) -> io::Result<Self> {
        let root = open_path(path, FILE_READ_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS)?;
        let root_info = file_info(&root)?;
        if root_info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(invalid("索引根不是目录"));
        }
        ensure_ntfs(&root)?;
        let root_id = file_id(&root_info);
        let root_path = final_path(&root)?;
        let volume_path = volume_prefix(&root_path)?;
        // 卷 GUID 路径不带结尾反斜杠，打开的是卷设备。只请求读取权限。
        let volume = open_path(volume_path, GENERIC_READ, 0)?;
        let state = query_journal(&volume)?;
        let checkpoint = EventCheckpoint {
            last_event_id: state.next as u64,
            stream_id: format!(
                "ntfs-usn-v1:{}:{:016x}:{root_id:016x}",
                volume_path.to_string_lossy().to_ascii_lowercase(),
                state.id
            ),
        };
        Ok(Self {
            volume,
            root,
            root_path,
            root_id,
            state,
            checkpoint,
        })
    }

    pub fn still_current(&self, path: &Path) -> bool {
        let same_root =
            open_path(path, FILE_READ_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS).and_then(|handle| {
                Ok(file_id(&file_info(&handle)?) == self.root_id
                    && final_path(&handle)? == self.root_path)
            });
        matches!(same_root, Ok(true))
            && query_journal(&self.volume)
                .is_ok_and(|state| state.id == self.state.id && state.contains(self.state.next))
    }

    pub fn replay(
        &self,
        mount: &Path,
        previous: &EventCheckpoint,
        cancel: &AtomicBool,
    ) -> io::Result<BTreeMap<PathBuf, bool>> {
        let start =
            i64::try_from(previous.last_event_id).map_err(|_| invalid("USN 检查点超出范围"))?;
        if previous.stream_id != self.checkpoint.stream_id || !self.state.contains(start) {
            return Err(invalid("USN 日志已轮换、截断或索引根已变化"));
        }
        if self.state.next - start > MAX_JOURNAL_SPAN {
            return Err(invalid("未回放的 USN 日志过大"));
        }
        let deadline = Instant::now() + REPLAY_TIMEOUT;
        let records = self.read_records(start, deadline, cancel)?;
        let mut changed = BTreeMap::<PathBuf, bool>::new();
        let mut parents = ParentPaths::new(self, &records);
        let mut checked_files = HashSet::new();
        for record in &records {
            check_budget(deadline, cancel)?;
            let directory = record.directory();
            if directory && record.id == self.root_id {
                // 根的名称、访问控制或重解析点变化时，过去的路径范围不再可信。
                if record.reason
                    & (RENAME
                        | USN_REASON_FILE_DELETE
                        | USN_REASON_REPARSE_POINT_CHANGE
                        | USN_REASON_SECURITY_CHANGE)
                    != 0
                {
                    return Err(invalid("USN 报告索引根已变化"));
                }
                continue;
            }

            // 删除的硬链接可能不是记录里的名称，剩余链接数也无法还原旧路径。
            if record.reason & USN_REASON_HARD_LINK_CHANGE != 0 {
                return Err(invalid("USN 无法确认变化前的硬链接路径"));
            }
            // 同一文件的元数据经硬链接共享。即使记录来自根外，别名也可能在根内。
            if !directory && record.reason & ALIAS_METADATA != 0 && checked_files.insert(record.id)
            {
                self.check_hard_links(record.id)?;
            }
            let absolute = parents.resolve(record.parent)?.join(&record.name);
            let relative = match absolute.strip_prefix(&self.root_path) {
                Ok(path) if !path.as_os_str().is_empty() => path,
                _ if directory
                    && record.reason & STRUCTURE != 0
                    && self.root_path.starts_with(&absolute) =>
                {
                    return Err(invalid("USN 报告索引根的上级目录已变化"));
                }
                _ => continue,
            };
            if record.reason & RENAME != 0 {
                check_rename_spelling(&absolute)?;
            }
            let recheck = directory && record.reason & STRUCTURE != 0;
            *changed.entry(mount.join(relative)).or_default() |= recheck;
            if changed.len() > MAX_PATHS {
                return Err(invalid("USN 涉及的索引路径过多"));
            }
        }
        if !self.still_current(mount) {
            return Err(invalid("USN 回放期间日志或索引根已变化"));
        }
        Ok(changed)
    }

    fn check_hard_links(&self, id: u64) -> io::Result<()> {
        match open_by_id(&self.root, id) {
            Ok(file) if file_info(&file)?.nNumberOfLinks > 1 => {
                Err(invalid("USN 涉及具有多个路径的文件，需要重新确认硬链接"))
            }
            Ok(_) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn read_records(
        &self,
        mut cursor: i64,
        deadline: Instant,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<Record>> {
        let mut records = Vec::new();
        let mut page = vec![0u8; 64 * 1024];
        while cursor < self.state.next {
            check_budget(deadline, cancel)?;
            let input = READ_USN_JOURNAL_DATA_V1 {
                StartUsn: cursor,
                ReasonMask: REASONS,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                BytesToWaitFor: 0,
                UsnJournalID: self.state.id,
                MinMajorVersion: 2,
                MaxMajorVersion: 2,
            };
            let size = control(
                &self.volume,
                FSCTL_READ_USN_JOURNAL,
                std::ptr::from_ref(&input).cast(),
                size_of::<READ_USN_JOURNAL_DATA_V1>() as u32,
                &mut page,
            )?;
            cursor = read_page(&page[..size], cursor, self.state.next, &mut records)?;
        }
        Ok(records)
    }
}

struct ParentPaths<'a> {
    journal: &'a Journal,
    deleted: HashMap<u64, &'a Record>,
    resolved: HashMap<u64, PathBuf>,
}

impl<'a> ParentPaths<'a> {
    fn new(journal: &'a Journal, records: &'a [Record]) -> Self {
        let deleted = records
            .iter()
            .filter(|record| record.directory() && record.reason & USN_REASON_FILE_DELETE != 0)
            .map(|record| (record.id, record))
            .collect();
        Self {
            journal,
            deleted,
            resolved: HashMap::from([(journal.root_id, journal.root_path.clone())]),
        }
    }

    fn resolve(&mut self, mut id: u64) -> io::Result<PathBuf> {
        let mut missing_dirs = Vec::new();
        let mut directory = loop {
            if let Some(path) = self.resolved.get(&id) {
                break path.clone();
            }
            match open_by_id(&self.journal.root, id) {
                Ok(handle) => {
                    if file_info(&handle)?.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                        return Err(invalid("USN 的父文件 ID 已不指向目录"));
                    }
                    let path = final_path(&handle)?;
                    if volume_prefix(&path)? != volume_prefix(&self.journal.root_path)? {
                        return Err(invalid("USN 的父目录不在原卷上"));
                    }
                    self.resolved.insert(id, path.clone());
                    break path;
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    // 父目录也被删除时，沿同次回放的删除记录找出原路径。
                    let record = self
                        .deleted
                        .get(&id)
                        .ok_or_else(|| invalid("USN 无法确定已删除父目录的位置"))?;
                    if missing_dirs.len() >= 512 || record.parent == id {
                        return Err(invalid("USN 的父目录链不完整"));
                    }
                    missing_dirs.push((id, &record.name));
                    id = record.parent;
                }
                Err(err) => return Err(err),
            }
        };
        for (id, name) in missing_dirs.into_iter().rev() {
            directory.push(name);
            self.resolved.insert(id, directory.clone());
        }
        Ok(directory)
    }
}

struct Record {
    id: u64,
    parent: u64,
    name: OsString,
    reason: u32,
    attrs: u32,
}

impl Record {
    fn directory(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
    }
}

fn read_page(page: &[u8], start: i64, end: i64, records: &mut Vec<Record>) -> io::Result<i64> {
    let malformed = || invalid("USN 记录的数据布局无效");
    let next = i64::from_le_bytes(field(page, 0).ok_or_else(malformed)?);
    if next <= start {
        return Err(invalid("USN 游标未推进"));
    }
    let mut bytes = &page[8..];
    let mut previous = None;
    while !bytes.is_empty() {
        let len = u32::from_le_bytes(field(bytes, 0).ok_or_else(malformed)?) as usize;
        if len < 60 || !len.is_multiple_of(8) {
            return Err(malformed());
        }
        let record = bytes.get(..len).ok_or_else(malformed)?;
        bytes = &bytes[len..];
        // 次版本可以在名称之前添加字段，名称位置使用记录中的偏移。
        if u16::from_le_bytes(field(record, 4).unwrap()) != 2 {
            return Err(invalid("USN 记录格式版本不支持"));
        }
        let usn = i64::from_le_bytes(field(record, 24).unwrap());
        if usn < start || usn >= next || previous.is_some_and(|prev| usn <= prev) {
            return Err(invalid("USN 记录编号不连续"));
        }
        previous = Some(usn);
        if usn >= end {
            break;
        }
        let offset = u16::from_le_bytes(field(record, 58).unwrap()) as usize;
        let length = u16::from_le_bytes(field(record, 56).unwrap()) as usize;
        if offset < 60 || length == 0 || length % 2 != 0 || records.len() >= MAX_RECORDS {
            return Err(malformed());
        }
        let encoded = record.get(offset..offset + length).ok_or_else(malformed)?;
        let wide: Vec<_> = encoded
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        if wide.iter().any(|unit| matches!(*unit, 0 | 47 | 58 | 92)) {
            return Err(malformed());
        }
        let name = OsString::from_wide(&wide);
        if name == "." || name == ".." {
            return Err(malformed());
        }
        records.push(Record {
            id: u64::from_le_bytes(field(record, 8).unwrap()),
            parent: u64::from_le_bytes(field(record, 16).unwrap()),
            name,
            reason: u32::from_le_bytes(field(record, 40).unwrap()),
            attrs: u32::from_le_bytes(field(record, 52).unwrap()),
        });
    }
    Ok(next)
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    bytes.get(offset..offset + N)?.try_into().ok()
}

struct JournalState {
    id: u64,
    first: i64,
    next: i64,
}

impl JournalState {
    fn contains(&self, usn: i64) -> bool {
        usn > 0 && (self.first..=self.next).contains(&usn)
    }
}

fn query_journal(volume: &OwnedHandle) -> io::Result<JournalState> {
    let mut out = [0u8; 128];
    let size = control(volume, FSCTL_QUERY_USN_JOURNAL, null(), 0, &mut out)?;
    if size < size_of::<USN_JOURNAL_DATA_V0>() {
        return Err(invalid("USN 日志头太短"));
    }
    // 安全性：Windows 已填充 V0 结构的前缀，所有字段均是整数，缓冲区可能未对齐。
    let data = unsafe { out.as_ptr().cast::<USN_JOURNAL_DATA_V0>().read_unaligned() };
    if size >= size_of::<USN_JOURNAL_DATA_V1>() {
        let data = unsafe { out.as_ptr().cast::<USN_JOURNAL_DATA_V1>().read_unaligned() };
        if !(data.MinSupportedMajorVersion..=data.MaxSupportedMajorVersion).contains(&2) {
            return Err(invalid("NTFS 日志不支持 USN V2"));
        }
    }
    let first = data.FirstUsn.max(data.LowestValidUsn);
    if data.UsnJournalID == 0 || data.NextUsn <= 0 || first < 0 || first > data.NextUsn {
        return Err(invalid("USN 日志范围无效"));
    }
    Ok(JournalState {
        id: data.UsnJournalID,
        first,
        next: data.NextUsn,
    })
}

fn control(
    volume: &OwnedHandle,
    code: u32,
    input: *const std::ffi::c_void,
    input_len: u32,
    output: &mut [u8],
) -> io::Result<usize> {
    let mut returned = 0;
    // 安全性：句柄存活，缓冲区大小由调用方提供。同步请求不传 OVERLAPPED。
    let ok = unsafe {
        DeviceIoControl(
            volume.as_raw_handle(),
            code,
            input,
            input_len,
            output.as_mut_ptr().cast(),
            output.len() as u32,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if returned as usize > output.len() {
        return Err(invalid("USN 响应超出缓冲区"));
    }
    Ok(returned as usize)
}

fn check_rename_spelling(path: &Path) -> io::Result<()> {
    let handle = match open_path(
        path,
        FILE_READ_ATTRIBUTES,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
    ) {
        Ok(handle) => handle,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    if final_path(&handle)?.file_name() != path.file_name() {
        return Err(invalid("USN 重命名后需要确认文件名拼写"));
    }
    Ok(())
}

fn open_path(path: &Path, access: u32, flags: u32) -> io::Result<OwnedHandle> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(invalid("Windows 路径包含 NUL"));
    }
    wide.push(0);
    // 安全性：路径以 NUL 结尾，不传可选安全属性和模板句柄。
    own_handle(unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            SHARE,
            null(),
            OPEN_EXISTING,
            flags,
            null_mut(),
        )
    })
}

fn open_by_id(volume: &OwnedHandle, id: u64) -> io::Result<OwnedHandle> {
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: FileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: id as i64 },
    };
    // 只查询属性和路径，访问模式用 0，避免为根外文件申请不需要的读权限。
    // 安全性：FileIdType 对应 union 的 FileId 字段，volume 在调用结束前有效。
    own_handle(unsafe {
        OpenFileById(
            volume.as_raw_handle(),
            &descriptor,
            0,
            SHARE,
            null(),
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    })
}

fn own_handle(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // 安全性：CreateFileW/OpenFileById 成功时返回由调用方关闭的句柄。
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn file_info(handle: &OwnedHandle) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // 安全性：输出结构可写，句柄在调用期间有效。
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

fn file_id(info: &BY_HANDLE_FILE_INFORMATION) -> u64 {
    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
}

fn final_path(handle: &OwnedHandle) -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 512];
    loop {
        // 安全性：缓冲区有指定数量的 UTF-16 码元，句柄有效。
        let len = unsafe {
            GetFinalPathNameByHandleW(
                handle.as_raw_handle(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                VOLUME_NAME_GUID | FILE_NAME_NORMALIZED,
            )
        } as usize;
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        if len < buf.len() {
            return Ok(PathBuf::from(OsString::from_wide(&buf[..len])));
        }
        if len >= 32_768 {
            return Err(invalid("USN 规范路径过长"));
        }
        buf.resize(len + 1, 0);
    }
}

fn volume_prefix(path: &Path) -> io::Result<&Path> {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return Err(invalid("未找到本地卷的 GUID"));
    };
    let text = prefix
        .as_os_str()
        .to_str()
        .ok_or_else(|| invalid("卷 GUID 编码无效"))?;
    if !text.starts_with(r"\\?\Volume{") || !text.ends_with('}') {
        return Err(invalid("USN 仅适用于本地 NTFS 卷"));
    }
    Ok(Path::new(prefix.as_os_str()))
}

fn ensure_ntfs(handle: &OwnedHandle) -> io::Result<()> {
    let mut name = [0u16; 32];
    // 安全性：仅需要文件系统名称，其他可选输出均为 NULL。
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if !name.starts_with(&[b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0]) {
        return Err(invalid("USN 回放仅支持本地 NTFS"));
    }
    Ok(())
}

fn check_budget(deadline: Instant, cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(io::ErrorKind::Interrupted.into());
    }
    if Instant::now() >= deadline {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "USN 回放超时"));
    }
    Ok(())
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(usn: i64, name: &str) -> Vec<u8> {
        let wide: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let len = (60 + wide.len()).next_multiple_of(8);
        let mut record = vec![0; len];
        record[..4].copy_from_slice(&(len as u32).to_le_bytes());
        record[4..6].copy_from_slice(&2u16.to_le_bytes());
        record[8..16].copy_from_slice(&7u64.to_le_bytes());
        record[16..24].copy_from_slice(&3u64.to_le_bytes());
        record[24..32].copy_from_slice(&usn.to_le_bytes());
        record[40..44].copy_from_slice(&USN_REASON_FILE_CREATE.to_le_bytes());
        record[52..56].copy_from_slice(&FILE_ATTRIBUTE_DIRECTORY.to_le_bytes());
        record[56..58].copy_from_slice(&(wide.len() as u16).to_le_bytes());
        record[58..60].copy_from_slice(&60u16.to_le_bytes());
        record[60..60 + wide.len()].copy_from_slice(&wide);
        record
    }

    #[test]
    fn reads_only_records_before_checkpoint() {
        let mut page = 400i64.to_le_bytes().to_vec();
        page.extend(record(150, "中文目录"));
        page.extend(record(300, "检查点之后"));
        let mut records = Vec::new();
        assert_eq!(read_page(&page, 100, 300, &mut records).unwrap(), 400);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "中文目录");
        assert_eq!((records[0].id, records[0].parent), (7, 3));
        assert!(records[0].directory());
    }

    #[test]
    fn rejects_truncation_invalid_names_and_stalled_cursor() {
        let mut page = 200i64.to_le_bytes().to_vec();
        page.extend(record(150, r"sub\file"));
        let mut records = Vec::new();
        assert!(read_page(&page, 100, 200, &mut records).is_err());
        assert!(read_page(&page[..page.len() - 1], 100, 200, &mut records).is_err());
        assert!(read_page(&page[..8], 200, 250, &mut records).is_err());
        assert!(records.is_empty());
    }
}
