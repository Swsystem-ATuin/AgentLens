use std::{
    collections::HashSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(super) struct Profiles {
    pub(super) codex: Vec<PathBuf>,
    pub(super) browser_profiles: Vec<PathBuf>,
}

pub(super) fn discover(profile: &Path, local: &Path, mask: bool) -> Profiles {
    let actual = profile.join("AppData");
    let mut roots = vec![actual.join("Local"), local.to_owned(), actual.join("Roaming")];
    if let Some(path) = super::override_path(&["APPDATA"]) {
        roots.push(path);
    }
    let mut codex = Vec::new();
    let mut browser_profiles = Vec::new();
    let mut codex_profiles = Vec::new();
    let mut chat_candidates = Vec::new();
    let mut seen_roots = HashSet::new();
    for root in roots {
        let key = location_key(&root);
        if !seen_roots.insert(key) { continue; }
        chat_candidates.push(root.join("ChatGPT"));
        codex_profiles.push(root.join("Codex"));
        codex_profiles.push(root.join("OpenAI/Codex"));
        let packages = root.join("Packages");
        match packages.read_dir() {
            Ok(entries) => {
                let mut entries: Vec<_> = entries.filter_map(|entry| match entry {
                    Ok(entry) => Some(entry),
                    Err(_) => { report("Windows packages", "access failed", &packages, mask); None }
                }).collect();
                entries.sort_unstable_by_key(fs::DirEntry::file_name);
                for entry in entries {
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    let roaming = entry.path().join("LocalCache/Roaming");
                    if name.starts_with("openai.chatgpt-desktop_") || name.starts_with("openai.chatgpt_") {
                        chat_candidates.push(roaming.join("ChatGPT"));
                    } else if name.starts_with("openai.codex_") {
                        codex_profiles.push(roaming.join("Codex"));
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => report("Windows packages", "access failed", &packages, mask),
        }
    }

    let mut seen = HashSet::new();
    let mut chat_found = false;
    for path in chat_candidates {
        let Some(path) = checked("ChatGPT browser profile", path, mask, &mut seen) else { continue; };
        browser_profiles.push(path.clone());
        let mut markers = Vec::new();
        for (name, label) in [
            ("IndexedDB", "IndexedDB"),
            ("Local Storage/leveldb", "local storage"),
            ("Session Storage", "browser session storage"),
        ] {
            match path.join(name).try_exists() {
                Ok(true) => markers.push(label),
                Ok(false) => {}
                Err(_) => report("ChatGPT browser profile", "access failed", &path.join(name), mask),
            }
        }
        if markers.is_empty() {
            report("ChatGPT browser profile", "supported history not found", &path, mask);
        } else {
            chat_found = true;
            report("ChatGPT browser profile", &format!("storage detected but unsupported ({})", markers.join(", ")), &path, mask);
        }
    }
    if !chat_found { println!("ChatGPT browser storage: not found in the checked profiles."); }

    let mut seen = HashSet::new();
    for candidate in codex_profiles {
        let Some(path) = checked("Codex app profile", candidate, mask, &mut seen) else { continue; };
        let mut found = false;
        for home in [path.clone(), path.join(".codex")] {
            match history_markers(&home) {
                Ok(true) => { codex.push(home); found = true; }
                Ok(false) => {}
                Err(_) => report("Codex app profile", "access failed", &home, mask),
            }
        }
        if !found { report("Codex app profile", "storage detected but no supported history found", &path, mask); }
    }
    Profiles { codex, browser_profiles }
}

fn checked(
    agent: &str,
    path: PathBuf,
    mask: bool,
    seen: &mut HashSet<String>,
) -> Option<PathBuf> {
    match path.try_exists() {
        Ok(true) if seen.insert(location_key(&path)) => {
            if super::directory_location(&path).is_ok() { Some(path) } else {
                report(agent, "access failed or unsupported linked location", &path, mask);
                None
            }
        }
        Ok(_) => None,
        Err(_) => { report(agent, "access failed", &path, mask); None }
    }
}

fn history_markers(path: &Path) -> std::io::Result<bool> {
    let entries = match path.read_dir() {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let name = entry?.file_name();
        let name = name.to_string_lossy();
        if ["sessions", "archived_sessions"].contains(&name.as_ref())
            || (name.ends_with(".sqlite") && (name.starts_with("state_") || name.starts_with("thread_history_")))
        {
            return Ok(true);
        }
    }
    // Two known optional catalogs. Browser SQLite files are never probed.
    for name in ["sqlite/codex.db", "sqlite/codex-dev.db"] {
        if path.join(name).try_exists()? { return Ok(true); }
    }
    Ok(false)
}

fn location_key(path: &Path) -> String {
    path.canonicalize().unwrap_or_else(|_| path.to_owned()).to_string_lossy().to_lowercase()
}

pub(super) fn report(agent: &str, state: &str, path: &Path, mask: bool) {
    if mask { println!("{agent}: {state}"); }
    else { println!("{agent}: {state} — {}", path.display()); }
}
