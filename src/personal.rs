use crate::{auth, db, ingest, projects, redaction, sources::Adapter, web};
use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    io::ErrorKind,
    mem::MaybeUninit,
    net::Ipv4Addr,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};
use windows_sys::Win32::{
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
        GetFileInformationByHandle,
    },
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};

pub(crate) async fn run(
    port: u16,
    index_only: bool,
    no_browser: bool,
    redaction_file: Option<&Path>,
) -> Result<()> {
    let homes = discover()?;
    let app_data = env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?;
    let storage = override_path(&["AGENT_LENS_LOCAL_DIR"])
        .unwrap_or_else(|| PathBuf::from(app_data).join("AgentLens"));
    let storage_location = directory_location(&storage)?;
    for (_, home) in &homes {
        let home = directory_location(home)?;
        ensure!(
            !overlaps(&storage_location, &home),
            "Personal storage and agent homes must be separate directories"
        );
    }

    fs::create_dir_all(&storage).context("create personal storage")?;
    // Match the source launcher's exclusive handle. The empty file can remain
    // between runs and no data is written to an existing lock file.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(storage.join("launcher.lock"))
        .context("cannot lock personal storage; close the other launcher and retry")?;
    ensure!(
        lock.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "Personal storage contains a linked launcher.lock file"
    );
    validate_storage(&storage)?;

    let listener = if index_only {
        None
    } else {
        Some(
            tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))
                .await
                .with_context(|| format!("cannot bind 127.0.0.1:{port}; try --port 3002"))?,
        )
    };
    for (adapter, _) in &homes {
        println!("Found {}", adapter.display_name());
    }
    println!("Rebuilding the personal usage index...");
    let database = storage.join("index.sqlite");
    crate::rebuild_index(
        &database,
        ingest::SourceInput::Personal(&homes),
        redaction_file,
    )
    .await?;
    let Some(listener) = listener else {
        println!("Personal index rebuilt.");
        return Ok(());
    };

    let mut state = auth::StateStore::open_with_mfa_key(
        &storage.join("state.sqlite"),
        Some(&storage.join("mfa-keys.json")),
    )
    .await?;
    state.enable_local_mode();
    let pool = db::open(&database).await?;
    redaction::Policy::load(redaction_file)?
        .validate_index(&pool)
        .await?;
    let projects_file = override_path(&["AGENT_LENS_PROJECTS_FILE"]);
    let projects = projects::ProjectConfig::load(projects_file.as_deref())?;
    let url = format!("http://127.0.0.1:{port}/usage");
    println!("Open {url} (Ctrl-C to stop; reopen AgentLens to update usage).");
    if !no_browser && !open_browser(&url) {
        eprintln!("Could not open the default browser. Copy the URL above.");
    }
    // The listener is already bound and stays in this process. Windows closes
    // the listener and releases the lock when the foreground process exits.
    web::serve_on(pool, listener, state, projects, false).await
}

fn discover() -> Result<Vec<(Adapter, PathBuf)>> {
    let profile = PathBuf::from(env::var_os("USERPROFILE").context("USERPROFILE is missing")?);
    let candidates: [(Adapter, &str, &[&str]); 3] = [
        (Adapter::Codex, ".codex", &["AGENT_LENS_CODEX_HOME", "CODEX_HOME"]),
        (Adapter::Gemini, ".gemini", &["AGENT_LENS_GEMINI_HOME"]),
        (Adapter::Hermes, ".hermes", &["AGENT_LENS_HERMES_HOME", "HERMES_HOME"]),
    ];
    let mut homes = Vec::new();
    let mut locations: Vec<PathBuf> = Vec::new();
    for (adapter, directory, variables) in candidates {
        let custom = override_path(variables);
        let path = custom.clone().unwrap_or_else(|| profile.join(directory));
        let location = directory_location(&path)?;
        if !path.try_exists()? {
            ensure!(
                custom.is_none(),
                "{} home does not exist",
                adapter.display_name()
            );
            continue;
        }
        ensure!(
            locations.iter().all(|other| !overlaps(&location, other)),
            "Configure separate, non-overlapping directories for each agent"
        );
        locations.push(location);
        homes.push((adapter, path));
    }
    ensure!(
        !homes.is_empty(),
        "No supported agent homes found in USERPROFILE; record activity or set a home override"
    );
    Ok(homes)
}

fn override_path(variables: &[&str]) -> Option<PathBuf> {
    variables
        .iter()
        .find_map(|key| env::var_os(key).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn directory_location(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute()
            && matches!(path.components().next(), Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_)))
            && path.components().all(|component| match component {
                Component::ParentDir => false,
                Component::Normal(part) => !part.to_string_lossy().ends_with(['.', ' ']),
                _ => true,
            }),
        "Use an absolute local drive path without parent components: {}",
        path.display()
    );
    let mut existing = None;
    for part in path.ancestors() {
        match part.symlink_metadata() {
            Ok(metadata) => {
                ensure!(
                    metadata.is_dir()
                        && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                    "Expected an ordinary directory without junctions or symbolic links: {}",
                    part.display()
                );
                existing.get_or_insert(part);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("inspect {}", part.display())),
        }
    }
    let existing = existing.context("local drive is unavailable")?;
    Ok(existing.canonicalize()?.join(path.strip_prefix(existing)?))
}

fn overlaps(a: &Path, b: &Path) -> bool {
    let a = a.to_string_lossy().to_lowercase();
    let b = b.to_string_lossy().to_lowercase();
    fn inside(path: &str, root: &str) -> bool {
        let root = root.trim_end_matches('\\');
        path.strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
    }
    inside(&a, &b) || inside(&b, &a)
}

fn validate_storage(storage: &Path) -> Result<()> {
    for entry in storage.read_dir()? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if ![
            "index.sqlite",
            "state.sqlite",
            "mfa-keys.json",
            ".index.sqlite.rebuild-",
        ]
        .iter()
        .any(|&prefix| name.starts_with(prefix))
        {
            continue;
        }
        let metadata = entry.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
                && link_count(&fs::File::open(entry.path())?)? == 1,
            "Expected a regular launcher file without other links: {}",
            entry.path().display()
        );
    }
    Ok(())
}

fn link_count(file: &fs::File) -> Result<u32> {
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // The open file owns the handle and the Win32 call initializes the output on success.
    let success =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if success == 0 {
        return Err(std::io::Error::last_os_error()).context("inspect launcher file hard links");
    }
    Ok(unsafe { information.assume_init() }.nNumberOfLinks)
}

fn open_browser(url: &str) -> bool {
    let url: Vec<u16> = url.encode_utf16().chain([0]).collect();
    // ShellExecuteW uses the user's default handler for this fixed loopback URL.
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            ptr::null(),
            url.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as isize > 32
}
