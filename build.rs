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
    let mut versions: Vec<_> = std::fs::read_dir(kits)
        .expect("Windows SDK is required for the ICE icon")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    versions.sort();
    let compiler = versions
        .iter()
        .rev()
        .map(|p| p.join("x64/rc.exe"))
        .find(|p| p.exists())
        .expect("Windows SDK x64/rc.exe not found");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let icon = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("brand/ice.ico");
    let resource = output.join("ice.rc");
    std::fs::write(
        &resource,
        format!(
            r#"1 ICON "{}"
1 VERSIONINFO
FILEVERSION 0,2,0,0
PRODUCTVERSION 0,2,0,0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "FileDescription", "ICE - Intent. Compile. Execute.\0"
   VALUE "ProductName", "ICE / FLOE Edition\0"
   VALUE "FileVersion", "0.2.0\0"
   VALUE "ProductVersion", "0.2.0\0"
   VALUE "OriginalFilename", "ice.exe\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x409, 1200
 END
END
"#,
            icon.display().to_string().replace('\\', "/")
        ),
    )
    .unwrap();
    let res = output.join("ice.res");
    let status = std::process::Command::new(compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(resource)
        .status()
        .unwrap();
    assert!(status.success(), "ICE Windows resource compilation failed");
    println!("cargo:rustc-link-arg={}", res.display());
}
