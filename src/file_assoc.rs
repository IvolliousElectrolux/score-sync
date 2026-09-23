//! 把 `.staffcrop` 登记成本程序打开 (当前用户, 不需要管理员).
//! Windows 双击工程文件时走命令行 `%1`.

#[cfg(windows)]
pub fn ensure_staffcrop_association() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = match exe.canonicalize() {
        Ok(p) => p,
        Err(_) => exe,
    };
    let exe = exe.to_string_lossy().replace('/', "\\");
    let cmd = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");
    if register_hkcu(&cmd, &icon).is_ok() {
        notify_assoc_changed();
    }
}

#[cfg(not(windows))]
pub fn ensure_staffcrop_association() {}

#[cfg(windows)]
fn register_hkcu(cmd: &str, icon: &str) -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (ext, _) = hkcu.create_subkey(r"Software\Classes\.staffcrop")?;
    let current: String = ext.get_value("").unwrap_or_default();
    if current != "ScoreSync.staffcrop" {
        ext.set_value("", &"ScoreSync.staffcrop")?;
    }

    let (prog, _) = hkcu.create_subkey(r"Software\Classes\ScoreSync.staffcrop")?;
    prog.set_value("", &"Score Sync 工程")?;
    let (icon_key, _) = prog.create_subkey("DefaultIcon")?;
    icon_key.set_value("", &icon)?;
    let (open, _) = prog.create_subkey(r"shell\open\command")?;
    let existing: String = open.get_value("").unwrap_or_default();
    if existing != cmd {
        open.set_value("", &cmd)?;
    }
    Ok(())
}

#[cfg(windows)]
fn notify_assoc_changed() {
    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
    const SHCNF_IDLIST: u32 = 0;
    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(
            event: i32,
            flags: u32,
            item1: *const std::ffi::c_void,
            item2: *const std::ffi::c_void,
        );
    }
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}
