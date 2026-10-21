fn main() {
    // Estilo fluent: os campos de texto seguem o tema claro/escuro escolhido no app (Palette.color-scheme
    // ligado ao Theme.dark no app.slint)
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("erro ao compilar as telas .slint");
    windows_resources();
}

/// Ícone, nome e versão do barerp.exe (Explorer, atalhos, barra de tarefas, "Propriedades > Detalhes").
/// Só age quando o alvo é Windows; do Mac (cargo-xwin) o embed-resource usa o llvm-rc, no Windows o rc.exe.
fn windows_resources() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon = format!("{dir}/assets/icon.ico").replace('\\', "/");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut nums = version.split(['.', '-']).map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0), nums.next().unwrap_or(0));
    // 0x0416 = português do Brasil, 1200 = Unicode
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "041604B0"
    BEGIN
      VALUE "CompanyName", "Pablo Franco"
      VALUE "FileDescription", "BarERP Pro"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "barerp"
      VALUE "LegalCopyright", "Pablo Franco"
      VALUE "OriginalFilename", "barerp.exe"
      VALUE "ProductName", "BarERP Pro"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0416, 1200
  END
END
"#
    );
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("barerp.rc");
    std::fs::write(&path, rc).expect("erro ao gravar barerp.rc");
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_optional()
        .expect("erro ao compilar o ícone do Windows (precisa do rc.exe ou do llvm-rc no PATH)");
}
