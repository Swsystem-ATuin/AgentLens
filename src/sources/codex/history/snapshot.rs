use anyhow::{Context, Result, ensure};
use libsqlite3_sys as sqlite;
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
};

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn read_handle(path: &Path, share: u32) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(share)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "linked SQLite file"));
    }
    Ok(file)
}

pub(super) fn make(source: &Path, target: &Path) -> Result<()> {
    // Offline recovery operates only on private copies. The main handle denies
    // write/delete access, and each existing sidecar is also held read-only.
    if let Ok(mut database) = read_handle(source, 1) {
        let mut files = Vec::new();
        for suffix in ["-wal", "-journal"] {
            match read_handle(&sidecar(source, suffix), 1) {
                Ok(file) => files.push((suffix, file)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("lock Codex SQLite sidecar for offline copy"),
            }
        }
        io::copy(&mut database, &mut File::create(target)?)?;
        for (suffix, mut file) in files {
            io::copy(&mut file, &mut File::create(sidecar(target, suffix))?)?;
        }
        return Ok(());
    }

    // The active source uses SQLite's normal WAL locks and a read-only SHM
    // mapping. Open the existing sidecars first and deny their deletion for
    // the duration so SQLite never has to create one in the source directory.
    let _wal = read_handle(&sidecar(source, "-wal"), 3)
        .context("active Codex SQLite has no accessible WAL; close Codex and retry")?;
    let _shm = read_handle(&sidecar(source, "-shm"), 3)
        .context("active Codex SQLite has no accessible shared-memory file; close Codex and retry")?;
    let source_uri = read_only_uri(source)?;
    let target_name = CString::new(target.to_str().context("non-Unicode snapshot path")?)?;
    let source_db = Database::open(&source_uri, sqlite::SQLITE_OPEN_READONLY | sqlite::SQLITE_OPEN_URI)?;
    let target_db = Database::open(&target_name, sqlite::SQLITE_OPEN_READWRITE | sqlite::SQLITE_OPEN_CREATE)?;
    // These handles remain open until the backup has been finished.
    let backup = unsafe {
        sqlite::sqlite3_backup_init(target_db.0, c"main".as_ptr(), source_db.0, c"main".as_ptr())
    };
    ensure!(!backup.is_null(), "initialize read-only Codex SQLite backup");
    let copied = unsafe { sqlite::sqlite3_backup_step(backup, -1) };
    let finished = unsafe { sqlite::sqlite3_backup_finish(backup) };
    ensure!(
        copied == sqlite::SQLITE_DONE && finished == sqlite::SQLITE_OK,
        "active Codex SQLite snapshot unavailable (SQLite result {copied}); close Codex and retry"
    );
    Ok(())
}

fn read_only_uri(path: &Path) -> Result<CString> {
    use std::fmt::Write;
    let path = path.to_str().context("non-Unicode Codex SQLite path")?;
    let mut uri = String::from("file:/");
    for byte in path.bytes() {
        match byte {
            b'\\' => uri.push('/'),
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'_' | b'-' => {
                uri.push(char::from(byte));
            }
            _ => write!(uri, "%{byte:02X}")?,
        }
    }
    uri.push_str("?mode=ro&readonly_shm=1");
    Ok(CString::new(uri)?)
}

struct Database(*mut sqlite::sqlite3);

impl Database {
    fn open(path: &CString, flags: i32) -> Result<Self> {
        let mut handle = ptr::null_mut();
        // sqlite3_open_v2 can allocate a handle even if it reports an error.
        let result = unsafe { sqlite::sqlite3_open_v2(path.as_ptr(), &mut handle, flags, ptr::null()) };
        let database = Self(handle);
        ensure!(result == sqlite::SQLITE_OK, "open Codex SQLite snapshot (SQLite result {result})");
        Ok(database)
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { sqlite::sqlite3_close(self.0) };
        }
    }
}
