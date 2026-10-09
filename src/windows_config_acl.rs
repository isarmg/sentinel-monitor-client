//! Restrict machine-wide credentials and recordings to the service account and administrators.
//!
//! `%ProgramData%` is readable by local users by default. A newly created config file
//! otherwise inherits that access, including its long-lived Server token and camera password.

use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io, iter,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr,
};

use anyhow::{Context, ensure};
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, LocalFree},
    Security::{
        ACL,
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
            SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, READ_CONTROL, WRITE_DAC,
    },
};

// D:P disables inheritance. The directory ACEs inherit to future descendants.
// LocalSystem is the account used by the MSI-installed service. Existing paths
// can have a non-administrator owner: OW removes that owner's implicit WRITE_DAC
// and grants only READ_CONTROL, so it cannot restore access after migration.
const DIRECTORY_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;RC;;;OW)";
const FILE_SDDL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;RC;;;OW)";
const MAX_RECORDING_ENTRIES: usize = 100_000;

struct PrivateAcl(PSECURITY_DESCRIPTOR, *mut ACL);

impl PrivateAcl {
    fn new(directory: bool) -> anyhow::Result<Self> {
        let sddl = wide(OsStr::new(if directory {
            DIRECTORY_SDDL
        } else {
            FILE_SDDL
        }));
        let mut descriptor = ptr::null_mut();
        // SAFETY: `sddl` is terminated and both out parameters are valid for this call.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error()).context("construct private Windows ACL");
        }
        let mut present = 0;
        let mut dacl = ptr::null_mut();
        let mut defaulted = 0;
        // SAFETY: Windows allocated `descriptor` on success; outputs are valid.
        let success = unsafe {
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
        };
        if success == 0 || present == 0 || dacl.is_null() {
            // SAFETY: `descriptor` came from ConvertStringSecurityDescriptor... .
            unsafe { LocalFree(descriptor) };
            anyhow::bail!("private Windows ACL did not contain a DACL");
        }
        Ok(Self(descriptor, dacl))
    }
}

impl Drop for PrivateAcl {
    fn drop(&mut self) {
        // SAFETY: descriptor is owned by this value and LocalFree is its documented free.
        unsafe { LocalFree(self.0) };
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(iter::once(0)).collect()
}

/// Administrative ACL initialization may encounter an existing public leaf.
/// Keep every physical ancestor pinned before opening or creating that leaf.
fn pin_parent(path: &Path) -> anyhow::Result<Vec<File>> {
    let absolute = std::path::absolute(path)?;
    let mut components = absolute.components();
    ensure!(
        matches!(components.next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
            && matches!(components.next(), Some(Component::RootDir)),
        "Windows private path must use a local drive"
    );
    ensure!(
        components.all(|part| matches!(part, Component::Normal(name) if !name.to_string_lossy().contains([':', '*', '?']) && !name.to_string_lossy().ends_with(['.', ' ']))),
        "Windows private path contains an unsafe component"
    );
    let parent = absolute
        .parent()
        .context("Windows private path has no parent")?;
    let mut handles = Vec::new();
    let mut current = PathBuf::new();
    for part in parent.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        let handle = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&current)
            .context("pin Windows private path ancestor")?;
        let metadata = handle.metadata()?;
        ensure!(
            metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "Windows private path ancestor is not a physical directory"
        );
        handles.push(handle);
        ensure!(
            handles.len() <= 256,
            "Windows private path exceeds ancestor budget"
        );
    }
    Ok(handles)
}

fn secure_path(path: &Path, directory: bool, acl: &PrivateAcl) -> anyhow::Result<()> {
    let absolute = std::path::absolute(path)?;
    let path = absolute.as_path();
    let _parents = pin_parent(path)?;
    let path_wide = wide(path.as_os_str());
    ensure!(
        !path_wide[..path_wide.len() - 1].contains(&0),
        "Windows ACL path contains a NUL"
    );
    // OPEN_REPARSE_POINT ensures a junction or symbolic link is inspected, never followed.
    // BACKUP_SEMANTICS is required to open directories using CreateFileW.
    let handle = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .context("open Windows path for administrative ACL restriction")?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: handle is open and info is a writable output buffer.
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("inspect private Windows path {}", path.display()));
    }
    ensure!(
        info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "refusing reparse point at private Windows path {}",
        path.display()
    );
    ensure!(
        (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) == directory,
        "private Windows path has unexpected file type: {}",
        path.display()
    );
    ensure!(
        directory || info.nNumberOfLinks == 1,
        "refusing multiply linked Windows credential or recording file"
    );
    // SAFETY: handle refers to the inspected file itself, and ACL points into a live descriptor.
    let result = unsafe {
        SetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl.1,
            ptr::null(),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32))
            .with_context(|| format!("restrict Windows access to {}", path.display()));
    }
    Ok(())
}

/// Create and restrict the common `%ProgramData%\\XcocClient` directory.
pub fn secure_config_directory(path: &Path) -> anyhow::Result<()> {
    let absolute = std::path::absolute(path)?;
    let path = absolute.as_path();
    let _parents = pin_parent(path)?;
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("create private Windows directory"),
    }
    let acl = PrivateAcl::new(true)?;
    secure_path(path, true, &acl)
}

/// Restrict an existing config file or its archived copy before reading it.
pub fn secure_config_file(path: &Path) -> anyhow::Result<()> {
    let acl = PrivateAcl::new(false)?;
    secure_path(path, false, &acl)
}

/// Restrict existing local recordings as well as the root used for future files.
///
/// Call once at service or setup startup, after the recording store layout is validated.
pub fn secure_recording_tree(root: &Path) -> anyhow::Result<()> {
    let absolute = std::path::absolute(root)?;
    let root = absolute.as_path();
    let parent = root.parent().context("recording root has no parent")?;
    secure_config_directory(parent)?;
    let _parents = pin_parent(root)?;
    match fs::create_dir(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("create private Windows recording root"),
    }
    let directory_acl = PrivateAcl::new(true)?;
    let file_acl = PrivateAcl::new(false)?;
    secure_path(root, true, &directory_acl)?;
    let mut entry_count = 0usize;
    for camera in fs::read_dir(root).with_context(|| format!("list {}", root.display()))? {
        let camera = camera?;
        entry_count += 1;
        ensure!(
            entry_count <= MAX_RECORDING_ENTRIES,
            "recording ACL migration exceeds entry limit"
        );
        secure_path(&camera.path(), true, &directory_acl)?;
        for recording in fs::read_dir(camera.path())? {
            entry_count += 1;
            ensure!(
                entry_count <= MAX_RECORDING_ENTRIES,
                "recording ACL migration exceeds entry limit"
            );
            secure_path(&recording?.path(), false, &file_acl)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
    };

    fn security_sddl(path: &Path, information: u32) -> String {
        let mut descriptor = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        let path_wide = wide(path.as_os_str());
        // SAFETY: valid NUL-terminated path and writable output pointers.
        let result = unsafe {
            GetNamedSecurityInfoW(
                path_wide.as_ptr() as *mut u16,
                SE_FILE_OBJECT,
                information,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(result, ERROR_SUCCESS);
        let mut text = ptr::null_mut();
        let mut len = 0;
        // SAFETY: descriptor is owned until LocalFree below; output pointers are valid.
        let result = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                information,
                &mut text,
                &mut len,
            )
        };
        assert_ne!(result, 0);
        // SAFETY: conversion succeeded and returned len UTF-16 code units.
        let sddl =
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len as usize) });
        // SAFETY: both allocations belong to their corresponding Win32 APIs.
        unsafe {
            LocalFree(text as *mut c_void);
            LocalFree(descriptor)
        };
        sddl
    }

    fn acl_sddl(path: &Path) -> String {
        security_sddl(path, DACL_SECURITY_INFORMATION)
    }

    #[test]
    fn multiply_linked_credentials_are_rejected_without_changing_the_shared_acl() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.json");
        let alias = temp.path().join("alias.json");
        fs::write(&original, b"unchanged credential").unwrap();
        fs::hard_link(&original, &alias).unwrap();
        let before = acl_sddl(&original);
        assert!(secure_config_file(&alias).is_err());
        assert_eq!(acl_sddl(&original), before);
        assert_eq!(fs::read(original).unwrap(), b"unchanged credential");
    }

    #[test]
    fn ancestor_junction_is_rejected_before_creation_or_acl_changes() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("unrelated");
        let junction = temp.path().join("junction");
        fs::create_dir(&outside).unwrap();
        let credential = outside.join("credential.json");
        fs::write(&credential, b"unrelated credential").unwrap();
        let before = acl_sddl(&credential);
        let status = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(secure_config_directory(&junction.join("must-not-exist")).is_err());
        assert!(secure_config_file(&junction.join("credential.json")).is_err());
        assert!(!outside.join("must-not-exist").exists());
        assert_eq!(acl_sddl(&credential), before);
        assert_eq!(fs::read(credential).unwrap(), b"unrelated credential");
    }

    #[test]
    fn config_and_existing_recordings_exclude_standard_users() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("XcocClient");
        let camera = root
            .join("recordings")
            .join("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        fs::create_dir_all(&camera).unwrap();
        let config = root.join("config.json");
        let recording = camera.join("clip.mp4");
        fs::write(&config, "secret").unwrap();
        fs::write(&recording, "clip").unwrap();
        let prior_owner = security_sddl(&recording, OWNER_SECURITY_INFORMATION);
        assert!(prior_owner.starts_with("O:"));
        secure_config_directory(&root).unwrap();
        secure_config_file(&config).unwrap();
        secure_recording_tree(&root.join("recordings")).unwrap();
        assert_eq!(
            security_sddl(&recording, OWNER_SECURITY_INFORMATION),
            prior_owner,
            "existing owner remains protected without requiring WRITE_OWNER"
        );
        for path in [&root, &config, &camera, &recording] {
            let acl = acl_sddl(path);
            assert!(acl.starts_with("D:P"), "{}: {acl}", path.display());
            assert!(acl.contains(";;;SY)"), "{}: {acl}", path.display());
            assert!(acl.contains(";;;BA)"), "{}: {acl}", path.display());
            assert!(acl.contains("RC;;;OW)"), "{}: {acl}", path.display());
            assert!(!acl.contains(";;;BU)"), "{}: {acl}", path.display());
            assert!(!acl.contains(";;;WD)"), "{}: {acl}", path.display());
            assert!(!acl.contains(";;;AU)"), "{}: {acl}", path.display());
        }
        let future_recording = camera.join("future.mp4");
        fs::write(&future_recording, "future clip").unwrap();
        let inherited_acl = acl_sddl(&future_recording);
        assert!(inherited_acl.contains(";;;OW)"), "{inherited_acl}");
        assert!(!inherited_acl.contains(";;;BU)"), "{inherited_acl}");
    }
}
