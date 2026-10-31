//! "Launch at Startup" integration.
//!
//! Writes or removes a value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. The entry starts
//! DM Ultra hidden in the tray (`--background`) so signing in does not open
//! the main window, mirroring Internet Download Manager.

/// Registry value name shown by Task Manager / Startup settings.
pub const VALUE_NAME: &str = "DM Ultra";

/// The command line stored in the Run key. Quoted because the install path
/// may contain spaces.
pub fn command_line() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\" --background", exe.display())
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    pub type Hkey = isize;

    pub const HKEY_CURRENT_USER: Hkey = -2147483647; // 0xFFFFFFFF80000001
    pub const KEY_QUERY_VALUE: u32 = 0x0001;
    pub const KEY_SET_VALUE: u32 = 0x0002;
    pub const REG_SZ: u32 = 1;
    pub const ERROR_SUCCESS: i32 = 0;
    pub const ERROR_FILE_NOT_FOUND: i32 = 2;
    pub const ERROR_MORE_DATA: i32 = 234;

    #[link(name = "advapi32")]
    extern "system" {
        pub fn RegCreateKeyExW(
            key: Hkey,
            sub_key: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam: u32,
            security: *const c_void,
            result: *mut Hkey,
            disposition: *mut u32,
        ) -> i32;
        pub fn RegOpenKeyExW(
            key: Hkey,
            sub_key: *const u16,
            options: u32,
            sam: u32,
            result: *mut Hkey,
        ) -> i32;
        pub fn RegSetValueExW(
            key: Hkey,
            value_name: *const u16,
            reserved: u32,
            kind: u32,
            data: *const u8,
            data_len: u32,
        ) -> i32;
        pub fn RegQueryValueExW(
            key: Hkey,
            value_name: *const u16,
            reserved: *const u32,
            kind: *mut u32,
            data: *mut u8,
            data_len: *mut u32,
        ) -> i32;
        pub fn RegDeleteValueW(key: Hkey, value_name: *const u16) -> i32;
        pub fn RegCloseKey(key: Hkey) -> i32;
    }

    pub fn sub_key() -> Vec<u16> {
        "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0"
            .encode_utf16()
            .collect()
    }

    pub fn value_name() -> Vec<u16> {
        super::VALUE_NAME.encode_utf16().chain(Some(0)).collect()
    }
}

/// Add or remove the Run entry to match `enabled`.
#[cfg(windows)]
pub fn sync(enabled: bool) -> std::io::Result<()> {
    use win::*;

    let sub_key = sub_key();
    let value = value_name();

    if enabled {
        let mut key: Hkey = 0;
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status));
        }
        let data: Vec<u16> = command_line().encode_utf16().chain(Some(0)).collect();
        let status = unsafe {
            RegSetValueExW(
                key,
                value.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * std::mem::size_of::<u16>()) as u32,
            )
        };
        unsafe {
            RegCloseKey(key);
        }
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status));
        }
        Ok(())
    } else {
        let mut key: Hkey = 0;
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(status));
        }
        let status = unsafe { RegDeleteValueW(key, value.as_ptr()) };
        unsafe {
            RegCloseKey(key);
        }
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status))
        }
    }
}

#[cfg(not(windows))]
pub fn sync(_enabled: bool) -> std::io::Result<()> {
    Ok(())
}

/// Current Run command line, if the entry exists.
#[cfg(windows)]
pub fn registered_command() -> Option<String> {
    use win::*;

    let sub_key = sub_key();
    let value = value_name();

    let mut key: Hkey = 0;
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sub_key.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    let mut len: u32 = 0;
    let mut kind: u32 = 0;
    let mut status = unsafe {
        RegQueryValueExW(
            key,
            value.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut len,
        )
    };
    if status == ERROR_SUCCESS && len > 0 {
        let mut buf = vec![0u8; len as usize + 2];
        let mut buf_len = buf.len() as u32;
        status = unsafe {
            RegQueryValueExW(
                key,
                value.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buf.as_mut_ptr(),
                &mut buf_len,
            )
        };
        unsafe {
            RegCloseKey(key);
        }
        if status == ERROR_SUCCESS || status == ERROR_MORE_DATA {
            let units: Vec<u16> = buf
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take_while(|unit| *unit != 0)
                .collect();
            return Some(String::from_utf16_lossy(&units));
        }
        return None;
    }

    unsafe {
        RegCloseKey(key);
    }
    None
}

#[cfg(not(windows))]
pub fn registered_command() -> Option<String> {
    None
}

/// True when the Run entry exists and points at this executable.
pub fn is_current() -> bool {
    registered_command()
        .map(|command| command.eq_ignore_ascii_case(&command_line()))
        .unwrap_or(false)
}
