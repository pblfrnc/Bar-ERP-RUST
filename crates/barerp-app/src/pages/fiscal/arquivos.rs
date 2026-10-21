//! Janelas nativas de abrir e salvar arquivo (o Slint não tem). macOS: AppleScript; Windows: diálogos do
//! .NET pelo PowerShell. Bloqueiam até o usuário escolher: chame de dentro de `spawn_blocking`.

use std::path::PathBuf;
use std::process::Command;

fn saida(cmd: &mut Command) -> Option<PathBuf> {
    let out = cmd.output().ok()?;
    let texto = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !texto.is_empty()).then(|| PathBuf::from(texto))
}

fn aspas_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn aspas_powershell(s: &str) -> String {
    s.replace('\'', "''")
}

/// Escolher um arquivo existente. `extensoes` sem ponto (ex.: `["pfx", "p12"]`). `None` se cancelou.
pub fn abrir(titulo: &str, extensoes: &[&str]) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let tipos = extensoes.iter().map(|e| format!("\"{e}\"")).collect::<Vec<_>>().join(", ");
        let script = format!(
            "POSIX path of (choose file with prompt \"{}\" of type {{{tipos}}})",
            aspas_applescript(titulo)
        );
        saida(Command::new("osascript").args(["-e", &script]))
    } else if cfg!(windows) {
        let filtro = extensoes.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";");
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; \
             $d.Title = '{}'; $d.Filter = 'Arquivos ({filtro})|{filtro}'; \
             if ($d.ShowDialog() -eq 'OK') {{ $d.FileName }}",
            aspas_powershell(titulo)
        );
        saida(Command::new("powershell").args(["-NoProfile", "-STA", "-Command", &script]))
    } else {
        saida(Command::new("zenity").args(["--file-selection", "--title", titulo]))
    }
}

/// Escolher onde salvar, já com o nome sugerido. `None` se cancelou.
pub fn salvar(titulo: &str, nome_sugerido: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let script = format!(
            "POSIX path of (choose file name with prompt \"{}\" default name \"{}\")",
            aspas_applescript(titulo),
            aspas_applescript(nome_sugerido)
        );
        saida(Command::new("osascript").args(["-e", &script]))
    } else if cfg!(windows) {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.SaveFileDialog; \
             $d.Title = '{}'; $d.FileName = '{}'; if ($d.ShowDialog() -eq 'OK') {{ $d.FileName }}",
            aspas_powershell(titulo),
            aspas_powershell(nome_sugerido)
        );
        saida(Command::new("powershell").args(["-NoProfile", "-STA", "-Command", &script]))
    } else {
        saida(Command::new("zenity").args(["--file-selection", "--save", "--filename", nome_sugerido]))
    }
}

/// Copia o texto para a área de transferência do sistema.
pub fn copiar(texto: &str) {
    crate::ctx::copy_text(texto);
}
