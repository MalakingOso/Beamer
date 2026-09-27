//! The local extraction server's lifecycle: stop, register, start. The
//! reconciler (`apply`) is the only caller, and nothing else in the app may
//! touch server lifecycle (see `agent_docs/local_inference.md`). Beamer
//! never spawns `llama-server.exe` itself; it only asks Task Scheduler to run
//! (or end) the task. Off Windows every call is a no-op (the catalog there is
//! empty, so none are reached).

use std::path::Path;

use anyhow::Result;

/// The one home for the task name. `installer/k2horizon/hooks.nsh`'s
/// uninstall section repeats it as `K2H_TASK_NAME` (NSIS can't read a Rust
/// constant), so change both together.
pub const TASK_NAME: &str = "Beamer K2-Horizon Server";

/// The task definition, ported from the XML `hooks.nsh` used to write. XML
/// rather than `schtasks /tr "..."`: `Command` and `Arguments` are separate
/// elements, so the launcher path needs no nested quoting. UTF-16LE with a
/// BOM: `schtasks /xml` refuses anything else ("unable to switch the
/// encoding"), even well-formed ASCII.
///
/// The logon trigger and the principal both name `user`. A bare
/// `<LogonTrigger>` means "at logon of *any* user", which only an admin may
/// register: Beamer runs unelevated, and `schtasks /create` refuses it with
/// "Access is denied" (the old installer hook got away with it only when run
/// elevated). Scoped to the current user, it registers and overwrites fine.
pub fn task_xml(launcher: &Path, user: &str) -> Vec<u8> {
    let launcher = xml_escape(&launcher.to_string_lossy());
    let user = xml_escape(user);
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\r\n\
         \x20 <Triggers>\r\n\
         \x20   <LogonTrigger>\r\n\
         \x20     <Enabled>true</Enabled>\r\n\
         \x20     <UserId>{user}</UserId>\r\n\
         \x20   </LogonTrigger>\r\n\
         \x20 </Triggers>\r\n\
         \x20 <Principals>\r\n\
         \x20   <Principal id=\"Author\">\r\n\
         \x20     <UserId>{user}</UserId>\r\n\
         \x20     <LogonType>InteractiveToken</LogonType>\r\n\
         \x20     <RunLevel>LeastPrivilege</RunLevel>\r\n\
         \x20   </Principal>\r\n\
         \x20 </Principals>\r\n\
         \x20 <Settings>\r\n\
         \x20   <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\r\n\
         \x20   <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\r\n\
         \x20   <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\r\n\
         \x20   <StartWhenAvailable>true</StartWhenAvailable>\r\n\
         \x20 </Settings>\r\n\
         \x20 <Actions Context=\"Author\">\r\n\
         \x20   <Exec>\r\n\
         \x20     <Command>wscript.exe</Command>\r\n\
         \x20     <Arguments>\"{launcher}\"</Arguments>\r\n\
         \x20   </Exec>\r\n\
         \x20 </Actions>\r\n\
         </Task>\r\n"
    );
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// `DOMAIN\user` for the account Beamer runs as (the machine name stands in
/// for the domain on a local account), which is what the task is scoped to.
pub fn current_user() -> String {
    let name = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// End the task and kill any `llama-server.exe` (its files are locked while
/// it runs). Failures are expected and ignored: usually nothing is running.
/// Kills every `llama-server.exe`, as the installer always did.
pub fn stop_server() {
    #[cfg(target_os = "windows")]
    {
        let _ = schtasks(&["/end", "/tn", TASK_NAME]);
        let _ = run_hidden("taskkill", &["/F", "/IM", "llama-server.exe", "/T"]);
        // Give the killed process a moment to release its handles before
        // the swap starts renaming its directory.
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Create or overwrite (`/f`) the task from the XML at `xml_path`.
pub fn register_task(name: &str, xml_path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    schtasks(&["/create", "/tn", name, "/xml", &xml_path.to_string_lossy(), "/f"])?;
    #[cfg(not(target_os = "windows"))]
    let _ = (name, xml_path);
    Ok(())
}

/// Run the task now. Only ever called once the whole group, model
/// included, is on disk: a server started before its model exists sits at
/// "loading" forever with no error.
pub fn start_server() -> Result<()> {
    #[cfg(target_os = "windows")]
    schtasks(&["/run", "/tn", TASK_NAME])?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn schtasks(args: &[&str]) -> Result<()> {
    let out = run_hidden("schtasks", args)?;
    if !out.status.success() {
        anyhow::bail!(
            "schtasks {} exited with {}: {}",
            args.first().copied().unwrap_or_default(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_hidden(program: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> String {
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "UTF-16LE BOM first");
        let units: Vec<u16> = bytes[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn the_task_runs_the_launcher_through_wscript_at_logon() {
        let xml = decode(&task_xml(
            Path::new(r"C:\Users\x\AppData\Local\Beamer\llama-k2horizon\start-llama-k2horizon-hidden.vbs"),
            r"BEARCAVE\x",
        ));
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-16\"?>\r\n"));
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<Command>wscript.exe</Command>"));
        assert!(xml.contains(
            r#"<Arguments>"C:\Users\x\AppData\Local\Beamer\llama-k2horizon\start-llama-k2horizon-hidden.vbs"</Arguments>"#
        ));
        assert!(xml.ends_with("</Task>\r\n"));
    }

    /// Unscoped, it's an any-user logon trigger, which an unelevated
    /// `schtasks /create` refuses with "Access is denied".
    #[test]
    fn the_logon_trigger_and_principal_are_scoped_to_the_user() {
        let xml = decode(&task_xml(Path::new(r"C:\l.vbs"), r"BEARCAVE\x"));
        let trigger = &xml[xml.find("<LogonTrigger>").unwrap()..xml.find("</LogonTrigger>").unwrap()];
        assert!(trigger.contains(r"<UserId>BEARCAVE\x</UserId>"));
        let principal = &xml[xml.find("<Principal ").unwrap()..xml.find("</Principal>").unwrap()];
        assert!(principal.contains(r"<UserId>BEARCAVE\x</UserId>"));
    }

    #[test]
    fn a_path_with_markup_characters_is_escaped() {
        let xml = decode(&task_xml(Path::new(r"C:\Users\A&B\launcher.vbs"), "x"));
        assert!(xml.contains(r"C:\Users\A&amp;B\launcher.vbs"));
    }
}
