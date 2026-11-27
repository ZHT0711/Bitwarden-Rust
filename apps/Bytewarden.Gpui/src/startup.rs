#[cfg(windows)]
mod platform {
    use std::{os::windows::ffi::OsStrExt, path::Path};

    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::{
            HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
            RegSetKeyValueW,
        },
    };

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE_NAME: &str = "Bytewarden.GPUI";

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn startup_command(executable: &Path) -> Option<Vec<u16>> {
        let mut command = Vec::with_capacity(executable.as_os_str().len() + 32);
        command.push('"' as u16);
        command.extend(executable.as_os_str().encode_wide());
        command.extend("\" --start-in-tray\0".encode_utf16());
        if command.len() > 32_767 {
            return None;
        }
        Some(command)
    }

    pub(super) fn set_enabled(enabled: bool) -> Result<(), String> {
        let key = wide(RUN_KEY);
        let name = wide(VALUE_NAME);
        let status = if enabled {
            let executable =
                std::env::current_exe().map_err(|error| format!("无法获取应用路径：{error}"))?;
            let command = startup_command(&executable)
                .ok_or_else(|| "应用启动命令超出 Windows 注册表限制。".to_string())?;
            let byte_length = u32::try_from(command.len() * std::mem::size_of::<u16>())
                .map_err(|_| "应用启动命令超出 Windows 注册表限制。".to_string())?;
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    command.as_ptr().cast(),
                    byte_length,
                )
            }
        } else {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) }
        };
        if status == ERROR_SUCCESS || (!enabled && status == ERROR_FILE_NOT_FOUND) {
            Ok(())
        } else {
            Err(format!("无法更新 Windows 自启动设置（错误 {status}）。"))
        }
    }

    fn command_matches(command: &[u16], executable: &Path) -> bool {
        startup_command(executable).is_some_and(|expected| command == expected)
    }

    pub(super) fn is_enabled() -> bool {
        let key = wide(RUN_KEY);
        let name = wide(VALUE_NAME);
        let mut value_type = 0;
        let mut byte_length = 0;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                &mut value_type,
                std::ptr::null_mut(),
                &mut byte_length,
            )
        };
        if status != ERROR_SUCCESS
            || value_type != REG_SZ
            || !(2..=65_534).contains(&byte_length)
            || !byte_length.is_multiple_of(2)
        {
            return false;
        }
        let mut command = vec![0u16; byte_length as usize / 2];
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                &mut value_type,
                command.as_mut_ptr().cast(),
                &mut byte_length,
            )
        };
        status == ERROR_SUCCESS
            && value_type == REG_SZ
            && byte_length.is_multiple_of(2)
            && (byte_length as usize / 2) <= command.len()
            && std::env::current_exe().is_ok_and(|executable| {
                command_matches(&command[..byte_length as usize / 2], &executable)
            })
    }

    #[cfg(test)]
    pub(super) fn matches_for_test(command: &[u16], executable: &Path) -> bool {
        command_matches(command, executable)
    }

    #[cfg(test)]
    pub(super) fn command_for_test(executable: &Path) -> Option<Vec<u16>> {
        startup_command(executable)
    }
}

#[cfg(not(windows))]
mod platform {
    pub(super) fn set_enabled(_: bool) -> Result<(), String> {
        Err("Windows 自启动仅在 Windows 上可用。".into())
    }

    pub(super) fn is_enabled() -> bool {
        false
    }
}

pub(crate) fn set_enabled(enabled: bool) -> Result<(), String> {
    platform::set_enabled(enabled)
}

pub(crate) fn is_enabled() -> bool {
    platform::is_enabled()
}

#[cfg(all(test, windows))]
mod tests {
    use super::platform::{command_for_test, matches_for_test};
    use std::path::Path;

    #[test]
    fn enabled_state_requires_this_executable_and_exact_supported_arguments() {
        let current = Path::new(r"C:\Program Files\Bytewarden\bytewarden-gpui.exe");
        let command = command_for_test(current).unwrap();
        assert!(matches_for_test(&command, current));
        assert!(!matches_for_test(
            &command,
            Path::new(r"D:\New Version\bytewarden-gpui.exe")
        ));
        assert!(!matches_for_test(&[], current));
        assert!(!matches_for_test(&[0], current));
        assert!(!matches_for_test(&command[..command.len() - 1], current));
        let extra =
            r#""C:\Program Files\Bytewarden\bytewarden-gpui.exe" --start-in-tray --unexpected"#
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>();
        assert!(!matches_for_test(&extra, current));
        let no_tray = r#""C:\Program Files\Bytewarden\bytewarden-gpui.exe""#
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>();
        assert!(!matches_for_test(&no_tray, current));
    }

    #[test]
    fn startup_command_quotes_the_executable_and_requests_tray_launch()
    -> Result<(), Box<dyn std::error::Error>> {
        let command = command_for_test(Path::new(
            r"C:\Program Files\Bytewarden\bytewarden-gpui.exe",
        ))
        .ok_or("bounded path")?;
        assert_eq!(
            String::from_utf16(&command[..command.len() - 1])?,
            r#""C:\Program Files\Bytewarden\bytewarden-gpui.exe" --start-in-tray"#
        );
        Ok(())
    }
}

#[cfg(test)]
mod platform_tests {
    #[cfg(not(windows))]
    #[test]
    fn startup_integration_is_unavailable_off_windows() {
        assert!(!super::is_enabled());
        assert!(super::set_enabled(true).is_err());
    }
}
