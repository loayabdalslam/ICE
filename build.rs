fn main() {
    println!("cargo:rerun-if-changed=brand/ice.ico");
    if !std::env::var("TARGET")
        .unwrap_or_default()
        .contains("windows-msvc")
    {
        return;
    }
    let kits = std::path::PathBuf::from(
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:/Program Files (x86)".into()),
    )
    .join("Windows Kits/10/bin");
    // The icon is cosmetic: never fail a source build over it.
    let Ok(dir) = std::fs::read_dir(kits) else {
        println!("cargo:warning=Windows SDK not found; building ice.exe without an icon");
        return;
    };
    let mut versions: Vec<_> = dir.filter_map(Result::ok).map(|e| e.path()).collect();
    versions.sort();
    let Some(compiler) = versions
        .iter()
        .rev()
        .map(|p| p.join("x64/rc.exe"))
        .find(|p| p.exists())
    else {
        println!("cargo:warning=rc.exe not found; building ice.exe without an icon");
        return;
    };
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let icon = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("brand/ice.ico");
    let resource = output.join("ice.rc");
    std::fs::write(
        &resource,
        format!(
            r#"1 ICON "{}"
1 VERSIONINFO
FILEVERSION {v},0
PRODUCTVERSION {v},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "FileDescription", "ICE - Intent. Compile. Execute.\0"
   VALUE "ProductName", "ICE / FLOE Edition\0"
   VALUE "FileVersion", "{ver}\0"
   VALUE "ProductVersion", "{ver}\0"
   VALUE "OriginalFilename", "ice.exe\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x409, 1200
 END
END
"#,
            icon.display().to_string().replace('\\', "/"),
            v = env!("CARGO_PKG_VERSION").replace('.', ","),
            ver = env!("CARGO_PKG_VERSION"),
        ),
    )
    .unwrap();
    let res = output.join("ice.res");
    let ok = std::process::Command::new(compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(resource)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg={}", res.display());
    } else {
        println!("cargo:warning=icon resource compilation failed; building without an icon");
    }
}
