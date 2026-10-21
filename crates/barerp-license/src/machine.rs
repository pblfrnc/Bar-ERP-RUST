//! ID da máquina igual ao `machineIdSync()` do pacote `node-machine-id` usado pelo Node: o GUID do sistema em
//! minúsculas, sem espaços, e depois o SHA-256 em hexadecimal. É esse ID que está cadastrado no painel de
//! licenças; se mudar, o cliente perde a licença na troca.

use sha2::{Digest, Sha256};

/// O que o Node grava quando não consegue ler o GUID.
pub const UNKNOWN: &str = "unknown-machine";

/// SHA-256 em hexadecimal do GUID já normalizado (`hash()` do `node-machine-id`).
pub fn hash_guid(guid: &str) -> String {
    hex::encode(Sha256::digest(guid.as_bytes()))
}

/// Saída do `REG QUERY ... /v MachineGuid` → GUID (`expose()` do `node-machine-id` no win32).
pub fn parse_windows_reg(output: &str) -> Option<String> {
    let after = output.split("REG_SZ").nth(1)?;
    let guid: String = after.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    (!guid.is_empty()).then_some(guid)
}

/// Saída do `ioreg -rd1 -c IOPlatformExpertDevice` → UUID (`expose()` no darwin).
pub fn parse_macos_ioreg(output: &str) -> Option<String> {
    let after = output.split("IOPlatformUUID").nth(1)?;
    let line = after.split('\n').next()?;
    let guid: String = line.chars().filter(|c| !(c.is_whitespace() || *c == '=' || *c == '"')).collect::<String>().to_lowercase();
    (!guid.is_empty()).then_some(guid)
}

/// GUID cru do sistema (sem o hash), como o `node-machine-id` lê.
pub fn raw_guid() -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Sem janela de console piscando (CREATE_NO_WINDOW)
        let windir = std::env::var("windir").unwrap_or_else(|_| r"C:\Windows".into());
        let output = std::process::Command::new(format!(r"{windir}\System32\REG.exe"))
            .args(["QUERY", r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?;
        parse_windows_reg(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("ioreg").args(["-rd1", "-c", "IOPlatformExpertDevice"]).output().ok()?;
        parse_macos_ioreg(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let text = std::fs::read_to_string("/var/lib/dbus/machine-id")
            .or_else(|_| std::fs::read_to_string("/etc/machine-id"))
            .ok()?;
        let guid: String = text.lines().next()?.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        (!guid.is_empty()).then_some(guid)
    }
}

/// ID desta máquina (o mesmo que o BarERP em Node mostrava e cadastrava no painel).
pub fn machine_id() -> String {
    raw_guid().map(|g| hash_guid(&g)).unwrap_or_else(|| UNKNOWN.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_reg_output() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n    MachineGuid    REG_SZ    3F2504E0-4F89-11D3-9A0C-0305E82C3301\r\n\r\n";
        assert_eq!(parse_windows_reg(out).as_deref(), Some("3f2504e0-4f89-11d3-9a0c-0305e82c3301"));
        assert_eq!(parse_windows_reg("erro"), None);
    }

    #[test]
    fn macos_ioreg_output() {
        let out = "+-o J314sAP  <class IOPlatformExpertDevice>\n    {\n      \"IOPlatformUUID\" = \"0A1B2C3D-AAAA-BBBB-CCCC-112233445566\"\n      \"IOPlatformSerialNumber\" = \"X\"\n";
        assert_eq!(parse_macos_ioreg(out).as_deref(), Some("0a1b2c3d-aaaa-bbbb-cccc-112233445566"));
    }

    #[test]
    fn hash_like_node() {
        // createHash('sha256').update('abc').digest('hex')
        assert_eq!(hash_guid("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
